mod db;
mod game;

use axum::{
    extract::{
        ws::{Message, WebSocket, WebSocketUpgrade},
        State,
    },
    response::IntoResponse,
    routing::get,
    Router,
};
use db::Db;
use game::combat::resolve_battle;
use game::data::*;
use game::shop::{ai_ladder_build, roll_shop};
use game::types::*;
use rand::Rng;
use serde::{Deserialize, Serialize};
use std::net::SocketAddr;
use std::sync::Arc;
use tower_http::services::ServeDir;

#[derive(Clone)]
struct AppState {
    db: Db,
}

#[derive(Debug, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
enum ClientMsg {
    NewRun { player_id: String, name: String },
    Resume { run_id: String },
    RenamePlayer { name: String },
    BuyCharacter { slot: usize },           // shop char slot
    BuyItem { slot: usize, target: usize }, // shop item slot, target team index
    BuyItemToSlot { slot: usize, target: usize, target_slot: ItemSlot },
    MoveItem { from_team: usize, from_slot: ItemSlot, to_team: usize, to_slot: ItemSlot },
    Sell { team_index: usize },             // sell whole character w/ items
    SellItem { team_index: usize, item_slot: ItemSlot },
    Reorder { from: usize, to: usize },
    Reroll,
    Battle,
    NextRound,    // returns to shop after battle
    Leaderboard { page: Option<usize>, per_page: Option<usize> },
}

#[derive(Debug, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
enum ServerMsg {
    Defs {
        characters: Vec<CharacterDef>,
        items: Vec<ItemDef>,
        constants: Constants,
    },
    State {
        run: Run,
    },
    Battle {
        opponent_name: String,
        events: Vec<game::combat::CombatEvent>,
        winner: Option<u8>,
        money_after: i32,
        phase: Phase,
        wins: i32,
        losses: i32,
        streak: i32,
        alive: bool,
    },
    Leaderboard {
        entries: Vec<LbEntry>,
        page: usize,
        page_count: usize,
    },
    Error {
        message: String,
    },
}

#[derive(Debug, Serialize)]
struct Constants {
    starting_money: i32,
    win_reward: i32,
    lose_reward: i32,
    max_losses: i32,
    reroll_cost: i32,
    max_team: usize,
}

#[derive(Debug, Serialize)]
struct LbEntry { name: String, streak: i32, wins: i32 }

const DEFAULT_LEADERBOARD_PAGE_SIZE: usize = 10;
const MAX_LEADERBOARD_PAGE_SIZE: usize = 50;

fn insufficient_money_msg(price: i32, wallet: i32) -> String {
    format!(
        "need ${} more — costs ${}, have ${}",
        (price - wallet).max(0),
        price,
        wallet
    )
}

fn insufficient_reroll_msg(wallet: i32) -> String {
    format!(
        "need ${} more to reroll — costs ${}, have ${}",
        (REROLL_COST - wallet).max(0),
        REROLL_COST,
        wallet
    )
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt().with_target(false).init();
    let db = Db::open("vaporslop.sqlite")?;
    let state = AppState { db };

    let app = Router::new()
        .route("/ws", get(ws_handler))
        .nest_service("/assets", ServeDir::new("assets"))
        .nest_service("/", ServeDir::new("static"))
        .with_state(Arc::new(state));

    let addr: SocketAddr = "0.0.0.0:3030".parse()?;
    tracing::info!("listening on http://{}", addr);
    let listener = tokio::net::TcpListener::bind(addr).await?;
    axum::serve(listener, app).await?;
    Ok(())
}

async fn ws_handler(ws: WebSocketUpgrade, State(state): State<Arc<AppState>>) -> impl IntoResponse {
    ws.on_upgrade(move |socket| handle_socket(socket, state))
}

async fn send(socket: &mut WebSocket, msg: &ServerMsg) {
    if let Ok(s) = serde_json::to_string(msg) {
        let _ = socket.send(Message::Text(s)).await;
    }
}

async fn handle_socket(mut socket: WebSocket, state: Arc<AppState>) {
    // Send defs immediately.
    send(&mut socket, &ServerMsg::Defs {
        characters: character_defs().to_vec(),
        items: item_defs().to_vec(),
        constants: Constants {
            starting_money: STARTING_MONEY,
            win_reward: WIN_REWARD,
            lose_reward: LOSE_REWARD,
            max_losses: MAX_LOSSES,
            reroll_cost: REROLL_COST,
            max_team: MAX_TEAM,
        },
    }).await;

    let mut current_run_id: Option<String> = None;

    while let Some(Ok(msg)) = socket.recv().await {
        let text = match msg {
            Message::Text(t) => t,
            Message::Close(_) => break,
            _ => continue,
        };
        let cmsg: ClientMsg = match serde_json::from_str(&text) {
            Ok(v) => v,
            Err(e) => { send(&mut socket, &ServerMsg::Error { message: format!("bad msg: {e}") }).await; continue; }
        };

        match cmsg {
            ClientMsg::NewRun { player_id, name } => {
                let run = Run {
                    id: uuid::Uuid::new_v4().to_string(),
                    player_id: clean_player_id(&player_id),
                    name: name.chars().take(24).collect(),
                    money: STARTING_MONEY,
                    wins: 0, losses: 0, streak: 0, alive: true,
                    best_streak: 0,
                    build: Build::default(),
                    shop: roll_shop(),
                    phase: Phase::Shop,
                };
                let cost = run.build.cost_value();
                if let Err(e) = state.db.upsert_run(&run, cost) {
                    send(&mut socket, &ServerMsg::Error { message: e.to_string() }).await;
                    continue;
                }
                current_run_id = Some(run.id.clone());
                send(&mut socket, &ServerMsg::State { run }).await;
            }
            ClientMsg::Resume { run_id } => {
                match state.db.load_run(&run_id) {
                    Ok(Some(run)) => { current_run_id = Some(run.id.clone()); send(&mut socket, &ServerMsg::State { run }).await; }
                    Ok(None) => send(&mut socket, &ServerMsg::Error { message: "run not found".into() }).await,
                    Err(e) => send(&mut socket, &ServerMsg::Error { message: e.to_string() }).await,
                }
            }
            ClientMsg::Leaderboard { page, per_page } => {
                match leaderboard_msg(&state, page, per_page) {
                    Ok(msg) => send(&mut socket, &msg).await,
                    Err(e) => send(&mut socket, &ServerMsg::Error { message: e }).await,
                }
            }
            other => {
                let id = match &current_run_id { Some(i) => i.clone(), None => { send(&mut socket, &ServerMsg::Error { message: "no active run".into() }).await; continue; } };
                let mut run = match state.db.load_run(&id) {
                    Ok(Some(r)) => r,
                    _ => { send(&mut socket, &ServerMsg::Error { message: "run gone".into() }).await; continue; }
                };
                let result = handle_run_action(&state, &mut run, other).await;
                let cost = run.build.cost_value();
                let _ = state.db.upsert_run(&run, cost);
                match result {
                    Ok(Some(extra)) => { send(&mut socket, &extra).await; send(&mut socket, &ServerMsg::State { run }).await; }
                    Ok(None) => send(&mut socket, &ServerMsg::State { run }).await,
                    Err(e) => send(&mut socket, &ServerMsg::Error { message: e }).await,
                }
            }
        }
    }
}

async fn handle_run_action(state: &AppState, run: &mut Run, msg: ClientMsg) -> Result<Option<ServerMsg>, String> {
    match msg {
        ClientMsg::BuyCharacter { slot } => {
            if run.phase != Phase::Shop { return Err("not in shop".into()); }
            let id = run.shop.characters.get(slot).and_then(|s| s.clone()).ok_or("slot empty")?;
            let def = character_def(&id).ok_or("unknown char")?;
            if run.money < def.cost {
                return Err(insufficient_money_msg(def.cost, run.money));
            }
            if run.build.team.len() >= MAX_TEAM { return Err("team full".into()); }
            run.money -= def.cost;
            run.build.team.push(TeamMember { def_id: id, hat: None, left_hand: None, right_hand: None });
            run.shop.characters[slot] = None;
            Ok(None)
        }
        ClientMsg::BuyItem { slot, target } => {
            if run.phase != Phase::Shop { return Err("not in shop".into()); }
            let id = run.shop.items.get(slot).and_then(|s| s.clone()).ok_or("slot empty")?;
            let def = item_def(&id).ok_or("unknown item")?;
            if run.money < def.cost {
                return Err(insufficient_money_msg(def.cost, run.money));
            }
            let member = run.build.team.get_mut(target).ok_or("no such team member")?;
            match def.slot {
                GearSlot::Hat => { if member.hat.is_some() { return Err("hat slot taken".into()); } member.hat = Some(id); }
                GearSlot::Hand => {
                    if member.left_hand.is_none() {
                        member.left_hand = Some(id);
                    } else if member.right_hand.is_none() {
                        member.right_hand = Some(id);
                    } else {
                        return Err("both hands full".into());
                    }
                }
            }
            run.money -= def.cost;
            run.shop.items[slot] = None;
            Ok(None)
        }
        ClientMsg::BuyItemToSlot { slot, target, target_slot } => {
            if run.phase != Phase::Shop { return Err("not in shop".into()); }
            let id = run.shop.items.get(slot).and_then(|s| s.clone()).ok_or("slot empty")?;
            let def = item_def(&id).ok_or("unknown item")?;
            if run.money < def.cost {
                return Err(insufficient_money_msg(def.cost, run.money));
            }
            if !slot_accepts(target_slot, def.slot) { return Err("wrong item socket".into()); }
            let member = run.build.team.get_mut(target).ok_or("no such team member")?;
            let dest = member_item_slot_mut(member, target_slot);
            if dest.is_some() { return Err("item socket taken".into()); }
            *dest = Some(id);
            run.money -= def.cost;
            run.shop.items[slot] = None;
            Ok(None)
        }
        ClientMsg::MoveItem { from_team, from_slot, to_team, to_slot } => {
            if run.phase != Phase::Shop { return Err("not in shop".into()); }
            if from_team >= run.build.team.len() || to_team >= run.build.team.len() {
                return Err("no such team member".into());
            }
            let item_id = member_item_slot(&run.build.team[from_team], from_slot)
                .as_ref()
                .cloned()
                .ok_or("no item there")?;
            let item_slot = item_def(&item_id).ok_or("unknown item")?.slot;
            if !slot_accepts(to_slot, item_slot) { return Err("wrong item socket".into()); }
            if member_item_slot(&run.build.team[to_team], to_slot).is_some() {
                return Err("item socket taken".into());
            }
            *member_item_slot_mut(&mut run.build.team[from_team], from_slot) = None;
            *member_item_slot_mut(&mut run.build.team[to_team], to_slot) = Some(item_id);
            Ok(None)
        }
        ClientMsg::Sell { team_index } => {
            if run.phase != Phase::Shop { return Err("not in shop".into()); }
            let m = run.build.team.get(team_index).ok_or("no such")?;
            let mut refund = character_def(&m.def_id).map(|d| d.cost).unwrap_or(0);
            for iid in m.item_ids() { refund += item_def(iid).map(|d| d.cost).unwrap_or(0); }
            run.money += (refund as f32 * SELL_RATIO) as i32;
            run.build.team.remove(team_index);
            Ok(None)
        }
        ClientMsg::SellItem { team_index, item_slot } => {
            if run.phase != Phase::Shop { return Err("not in shop".into()); }
            let member = run.build.team.get_mut(team_index).ok_or("no such")?;
            let slot = member_item_slot_mut(member, item_slot);
            let item_id = slot.take().ok_or("no item there")?;
            let refund = item_def(&item_id).map(|d| d.cost).unwrap_or(0);
            run.money += (refund as f32 * SELL_RATIO) as i32;
            Ok(None)
        }
        ClientMsg::Reorder { from, to } => {
            if from >= run.build.team.len() || to >= run.build.team.len() { return Err("oob".into()); }
            let m = run.build.team.remove(from);
            run.build.team.insert(to, m);
            Ok(None)
        }
        ClientMsg::Reroll => {
            if run.phase != Phase::Shop { return Err("not in shop".into()); }
            if run.money < REROLL_COST {
                return Err(insufficient_reroll_msg(run.money));
            }
            run.money -= REROLL_COST;
            run.shop = roll_shop();
            Ok(None)
        }
        ClientMsg::Battle => {
            if run.phase != Phase::Shop { return Err("not in shop".into()); }
            if run.build.team.is_empty() { return Err("no team".into()); }
            let target_gold = total_earned_gold(run);
            let opponent = state.db.find_opponent(&run.id, target_gold).map_err(|e| e.to_string())?;
            let (op_name, op_build_raw) = match opponent {
                Some((_id, name, b)) => (name, b),
                None => (synthetic_opponent_name(), ai_ladder_build(target_gold.max(50))),
            };
            let res = resolve_battle(&run.build, &op_build_raw);
            let won = res.winner == Some(0);
            if won {
                run.money += WIN_REWARD;
                run.wins += 1;
                run.streak += 1;
                run.best_streak = run.best_streak.max(run.streak);
            } else {
                run.money += LOSE_REWARD;
                run.losses += 1;
                run.streak = 0;
            }
            let _ = state.db.record_score(&run.player_id, &run.name, run.best_streak, run.wins);
            if run.losses >= MAX_LOSSES {
                run.alive = false;
                run.phase = Phase::GameOver;
            } else {
                run.phase = Phase::Battle; // will move back to shop on NextRound
                // Reroll shop for next round (only if continuing)
                run.shop = roll_shop();
            }
            Ok(Some(ServerMsg::Battle {
                opponent_name: op_name,
                events: res.events,
                winner: res.winner,
                money_after: run.money,
                phase: run.phase,
                wins: run.wins,
                losses: run.losses,
                streak: run.streak,
                alive: run.alive,
            }))
        }
        ClientMsg::NextRound => {
            if run.phase == Phase::Battle {
                if run.losses >= MAX_LOSSES {
                    run.phase = Phase::GameOver;
                    run.alive = false;
                } else {
                    run.phase = Phase::Shop;
                }
            }
            Ok(None)
        }
        ClientMsg::RenamePlayer { name } => {
            let name: String = name.chars().take(24).collect();
            if name.trim().is_empty() { return Err("name required".into()); }
            run.name = name;
            Ok(None)
        }
        ClientMsg::Leaderboard { page, per_page } => {
            leaderboard_msg(state, page, per_page).map(Some)
        }
        _ => Err("unhandled".into()),
    }
}

fn clean_player_id(player_id: &str) -> String {
    let id = player_id.trim();
    if id.is_empty() {
        uuid::Uuid::new_v4().to_string()
    } else {
        id.chars().take(64).collect()
    }
}

fn leaderboard_msg(state: &AppState, page: Option<usize>, per_page: Option<usize>) -> Result<ServerMsg, String> {
    let per_page = per_page
        .unwrap_or(DEFAULT_LEADERBOARD_PAGE_SIZE)
        .clamp(1, MAX_LEADERBOARD_PAGE_SIZE);
    let page = page.unwrap_or(1).max(1);
    let (entries, page_count) = state.db.leaderboard(page, per_page).map_err(|e| e.to_string())?;
    Ok(ServerMsg::Leaderboard {
        entries: entries.into_iter().map(|(name, streak, wins)| LbEntry { name, streak, wins }).collect(),
        page: page.min(page_count),
        page_count,
    })
}

fn total_earned_gold(run: &Run) -> i32 {
    STARTING_MONEY + run.wins * WIN_REWARD + run.losses * LOSE_REWARD
}

fn synthetic_opponent_name() -> String {
    const TAGS: &[&str] = &[
        "aesthet1c", "vapor", "moonbeam", "y2k", "memehead", "cybr", "lofi", "pix3l", "tokr",
        "dolphin", "neon", "glitch",
    ];
    let mut rng = rand::thread_rng();
    format!(
        "{}_bot_{:03}",
        TAGS[rng.gen_range(0..TAGS.len())],
        rng.gen_range(1..=999)
    )
}

fn member_item_slot(member: &TeamMember, slot: ItemSlot) -> &Option<String> {
    match slot {
        ItemSlot::Hat => &member.hat,
        ItemSlot::LeftHand => &member.left_hand,
        ItemSlot::RightHand => &member.right_hand,
    }
}

fn member_item_slot_mut(member: &mut TeamMember, slot: ItemSlot) -> &mut Option<String> {
    match slot {
        ItemSlot::Hat => &mut member.hat,
        ItemSlot::LeftHand => &mut member.left_hand,
        ItemSlot::RightHand => &mut member.right_hand,
    }
}

fn slot_accepts(target: ItemSlot, item: GearSlot) -> bool {
    match item {
        GearSlot::Hat => target == ItemSlot::Hat,
        GearSlot::Hand => {
            target == ItemSlot::LeftHand || target == ItemSlot::RightHand
        }
    }
}
