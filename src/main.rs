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
use game::shop::roll_shop;
use game::types::*;
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
    NewRun { name: String },
    Resume { run_id: String },
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
    Leaderboard,
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
    },
    Leaderboard {
        entries: Vec<LbEntry>,
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
            ClientMsg::NewRun { name } => {
                let run = Run {
                    id: uuid::Uuid::new_v4().to_string(),
                    name: name.chars().take(24).collect(),
                    money: STARTING_MONEY,
                    wins: 0, losses: 0, streak: 0, alive: true,
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
            if run.money < def.cost { return Err("not enough money".into()); }
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
            if run.money < def.cost { return Err("not enough money".into()); }
            let member = run.build.team.get_mut(target).ok_or("no such team member")?;
            match def.slot {
                ItemSlot::Hat => { if member.hat.is_some() { return Err("hat slot taken".into()); } member.hat = Some(id); }
                ItemSlot::LeftHand | ItemSlot::RightHand => {
                    // Hand items fill the matching hand first, falling back to the other hand.
                    let prefer_left = def.slot == ItemSlot::LeftHand;
                    let (first, second) = if prefer_left {
                        (&mut member.left_hand, &mut member.right_hand)
                    } else {
                        (&mut member.right_hand, &mut member.left_hand)
                    };
                    if first.is_none() { *first = Some(id); }
                    else if second.is_none() { *second = Some(id); }
                    else { return Err("both hands full".into()); }
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
            if run.money < def.cost { return Err("not enough money".into()); }
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
            if run.money < REROLL_COST { return Err("can't afford reroll".into()); }
            run.money -= REROLL_COST;
            run.shop = roll_shop();
            Ok(None)
        }
        ClientMsg::Battle => {
            if run.phase != Phase::Shop { return Err("not in shop".into()); }
            if run.build.team.is_empty() { return Err("no team".into()); }
            let target_lifetime_gold = lifetime_gold_earned(run);
            let opponent = state.db.find_opponent(target_lifetime_gold).map_err(|e| e.to_string())?;
            let (op_name, op_build_raw) = match opponent {
                Some((_id, name, b)) => (name, b),
                None => ("ghost".to_string(), game::shop::random_build(target_lifetime_gold.max(50))),
            };
            // Reverse opponent's team so their stored team[0] (their front) appears
            // on the far side of the right team, matching mirrored UI orientation.
            let op_build = Build { team: op_build_raw.team.into_iter().rev().collect() };
            let res = resolve_battle(&run.build, &op_build);
            let won = res.winner == Some(0);
            if won {
                run.money += WIN_REWARD;
                run.wins += 1;
                run.streak += 1;
            } else {
                run.money += LOSE_REWARD;
                run.losses += 1;
                run.streak = 0;
            }
            if run.losses >= MAX_LOSSES {
                run.alive = false;
                run.phase = Phase::GameOver;
                let _ = state.db.record_streak(&run.name, run.wins, run.wins);
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
            }))
        }
        ClientMsg::NextRound => {
            if run.phase == Phase::Battle { run.phase = Phase::Shop; }
            Ok(None)
        }
        ClientMsg::Leaderboard => {
            let entries = state.db.leaderboard().map_err(|e| e.to_string())?
                .into_iter().map(|(name, streak, wins)| LbEntry { name, streak, wins }).collect();
            Ok(Some(ServerMsg::Leaderboard { entries }))
        }
        _ => Err("unhandled".into()),
    }
}

fn lifetime_gold_earned(run: &Run) -> i32 {
    STARTING_MONEY + run.wins * WIN_REWARD + run.losses * LOSE_REWARD
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

fn slot_accepts(target: ItemSlot, item: ItemSlot) -> bool {
    match item {
        ItemSlot::Hat => target == ItemSlot::Hat,
        ItemSlot::LeftHand | ItemSlot::RightHand => {
            target == ItemSlot::LeftHand || target == ItemSlot::RightHand
        }
    }
}
