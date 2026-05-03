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
use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::Arc;
use tokio::sync::broadcast;
use tower_http::services::ServeDir;

#[derive(Clone)]
struct AppState {
    db: Db,
    presence: Arc<Presence>,
    stats_tx: broadcast::Sender<SiteStats>,
}

#[derive(Default)]
struct Presence {
    /// Refcount of open sockets per `player_id` (multi-tab).
    players: parking_lot::Mutex<HashMap<String, u32>>,
}

impl Presence {
    fn join(&self, id: &str) {
        let mut g = self.players.lock();
        *g.entry(id.to_string()).or_insert(0) += 1;
    }

    fn leave(&self, id: &str) {
        let mut g = self.players.lock();
        if let Some(c) = g.get_mut(id) {
            *c = c.saturating_sub(1);
            if *c == 0 {
                g.remove(id);
            }
        }
    }

    fn active_unique(&self) -> u32 {
        self.players.lock().len() as u32
    }
}

#[derive(Debug, Clone, Serialize)]
struct SiteStats {
    active_players: u32,
    logged_in_today: u32,
}

fn site_stats_snapshot(state: &AppState) -> SiteStats {
    SiteStats {
        active_players: state.presence.active_unique(),
        logged_in_today: state
            .db
            .count_players_logged_in_today()
            .unwrap_or(0),
    }
}

fn broadcast_site_stats(state: &AppState) {
    let _ = state.stats_tx.send(site_stats_snapshot(state));
}

fn register_socket_player(state: &AppState, reg: &mut Option<String>, player_id: &str) {
    if reg.is_none() {
        state.presence.join(player_id);
        if let Err(e) = state.db.touch_player_daily(player_id) {
            tracing::warn!("touch_player_daily: {e}");
        }
        *reg = Some(player_id.to_string());
        broadcast_site_stats(state);
    }
}

#[derive(Debug, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
enum ClientMsg {
    NewRun {
        player_id: String,
        name: String,
    },
    Resume {
        player_id: String,
    },
    RenamePlayer {
        name: String,
    },
    BuyCharacter {
        slot: usize,
        target: usize,
    }, // shop char slot, insert index (0..=team.len)
    BuyItem {
        slot: usize,
        target: usize,
    }, // shop item slot, target team index
    BuyItemToSlot {
        slot: usize,
        target: usize,
        target_slot: ItemSlot,
    },
    MoveItem {
        from_team: usize,
        from_slot: ItemSlot,
        to_team: usize,
        to_slot: ItemSlot,
    },
    Sell {
        team_index: usize,
    }, // sell whole character w/ items
    SellItem {
        team_index: usize,
        item_slot: ItemSlot,
    },
    Reorder {
        from: usize,
        to: usize,
    },
    Reroll,
    Battle,
    NextRound, // returns to shop after battle
    Leaderboard {
        page: Option<usize>,
        per_page: Option<usize>,
        around_player_id: Option<String>,
    },
}

#[derive(Debug, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
enum ServerMsg {
    Defs {
        characters: Vec<CharacterDef>,
        items: Vec<ItemDef>,
        constants: Constants,
        site_stats: SiteStats,
    },
    SiteStats {
        active_players: u32,
        logged_in_today: u32,
    },
    State {
        run: Run,
    },
    Battle {
        run_id: String,
        opponent_name: String,
        player_mmr_before: i32,
        opponent_mmr_before: Option<i32>,
        events: Vec<game::combat::CombatEvent>,
        run: Run,
        winner: Option<u8>,
        money_after: i32,
        phase: Phase,
        wins: i32,
        losses: i32,
        alive: bool,
    },
    Leaderboard {
        entries: Vec<LbEntry>,
        page: usize,
        page_count: usize,
        per_page: usize,
        player_rank: Option<usize>,
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
    max_wins: i32,
    reroll_cost: i32,
    max_team: usize,
}

#[derive(Debug, Serialize)]
struct LbEntry {
    player_id: String,
    name: String,
    wins: i32,
    mmr: i32,
}

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
    let (stats_tx, _) = broadcast::channel::<SiteStats>(64);
    let state = Arc::new(AppState {
        db,
        presence: Arc::new(Presence::default()),
        stats_tx,
    });

    let app = Router::new()
        .route("/ws", get(ws_handler))
        .nest_service("/assets", ServeDir::new("assets"))
        .nest_service("/", ServeDir::new("static"))
        .with_state(state);

    let port: u16 = std::env::var("PORT")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(3089);
    let addr = SocketAddr::from(([0, 0, 0, 0], port));
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
    let mut registered_player: Option<String> = None;
    let mut current_run_id: Option<String> = None;
    let mut rx = state.stats_tx.subscribe();

    send(
        &mut socket,
        &ServerMsg::Defs {
            characters: character_defs().to_vec(),
            items: item_defs().to_vec(),
            constants: Constants {
                starting_money: STARTING_MONEY,
                win_reward: WIN_REWARD,
                lose_reward: LOSE_REWARD,
                max_losses: MAX_LOSSES,
                max_wins: MAX_WINS,
                reroll_cost: REROLL_COST,
                max_team: MAX_TEAM,
            },
            site_stats: site_stats_snapshot(&state),
        },
    )
    .await;

    loop {
        tokio::select! {
            recv_msg = socket.recv() => {
                let Some(result) = recv_msg else { break };
                let msg = match result {
                    Ok(m) => m,
                    Err(_) => break,
                };
                let text = match msg {
                    Message::Text(t) => t,
                    Message::Close(_) => break,
                    _ => continue,
                };
                let cmsg: ClientMsg = match serde_json::from_str(&text) {
                    Ok(v) => v,
                    Err(e) => {
                        send(
                            &mut socket,
                            &ServerMsg::Error {
                                message: format!("bad msg: {e}"),
                            },
                        )
                        .await;
                        continue;
                    }
                };

                match cmsg {
                    ClientMsg::NewRun { player_id, name } => {
                        let player_id = clean_player_id(&player_id);
                        let mmr = match state.db.player_mmr(&player_id) {
                            Ok(Some(mmr)) => mmr,
                            Ok(None) => STARTING_MMR,
                            Err(e) => {
                                send(
                                    &mut socket,
                                    &ServerMsg::Error {
                                        message: e.to_string(),
                                    },
                                )
                                .await;
                                continue;
                            }
                        };
                        let run = Run {
                            id: uuid::Uuid::new_v4().to_string(),
                            player_id,
                            name: name.chars().take(24).collect(),
                            money: STARTING_MONEY,
                            wins: 0,
                            losses: 0,
                            streak: 0,
                            alive: true,
                            best_streak: 0,
                            mmr,
                            build: Build::default(),
                            shop: roll_shop(),
                            phase: Phase::Shop,
                        };
                        let cost = run.build.cost_value();
                        if let Err(e) = state.db.upsert_run(&run, cost) {
                            send(
                                &mut socket,
                                &ServerMsg::Error {
                                    message: e.to_string(),
                                },
                            )
                            .await;
                            continue;
                        }
                        register_socket_player(&state, &mut registered_player, &run.player_id);
                        current_run_id = Some(run.id.clone());
                        send(&mut socket, &ServerMsg::State { run }).await;
                    }
                    ClientMsg::Resume { player_id } => {
                        let player_id = clean_player_id(&player_id);
                        register_socket_player(&state, &mut registered_player, &player_id);
                        match state.db.load_latest_run_for_player(&player_id) {
                            Ok(Some(run)) => {
                                current_run_id = Some(run.id.clone());
                                send(&mut socket, &ServerMsg::State { run }).await;
                            }
                            Ok(None) => {
                                send(
                                    &mut socket,
                                    &ServerMsg::Error {
                                        message: "run not found".into(),
                                    },
                                )
                                .await
                            }
                            Err(e) => {
                                send(
                                    &mut socket,
                                    &ServerMsg::Error {
                                        message: e.to_string(),
                                    },
                                )
                                .await
                            }
                        }
                    }
                    ClientMsg::Leaderboard { page, per_page, around_player_id } => {
                        match leaderboard_msg(&state, page, per_page, around_player_id.as_deref()) {
                            Ok(msg) => send(&mut socket, &msg).await,
                            Err(e) => send(&mut socket, &ServerMsg::Error { message: e }).await,
                        }
                    }
                    other => {
                        let id = match &current_run_id {
                            Some(i) => i.clone(),
                            None => {
                                send(
                                    &mut socket,
                                    &ServerMsg::Error {
                                        message: "no active run".into(),
                                    },
                                )
                                .await;
                                continue;
                            }
                        };
                        let mut run = match state.db.load_run(&id) {
                            Ok(Some(r)) => r,
                            _ => {
                                send(
                                    &mut socket,
                                    &ServerMsg::Error {
                                        message: "run gone".into(),
                                    },
                                )
                                .await;
                                continue;
                            }
                        };
                        let is_battle = matches!(&other, ClientMsg::Battle);
                        let result = handle_run_action(&state, &mut run, other).await;
                        match result {
                            Ok(extra) => {
                                let cost = run.build.cost_value();
                                let save_result = if is_battle {
                                    let update_mmr = matches!(
                                        &extra,
                                        Some(ServerMsg::Battle {
                                            opponent_mmr_before: Some(_),
                                            ..
                                        })
                                    );
                                    state.db.record_score_and_upsert_run(&run, cost, update_mmr)
                                } else {
                                    state.db.upsert_run(&run, cost)
                                };
                                if let Err(e) = save_result {
                                    send(
                                        &mut socket,
                                        &ServerMsg::Error {
                                            message: e.to_string(),
                                        },
                                    )
                                    .await;
                                    continue;
                                }
                                if let Some(extra) = extra {
                                    send(&mut socket, &extra).await;
                                    if is_battle {
                                        continue;
                                    }
                                }
                                send(&mut socket, &ServerMsg::State { run }).await;
                            }
                            Err(e) => send(&mut socket, &ServerMsg::Error { message: e }).await,
                        }
                    }
                }
            }
            recv_st = rx.recv() => {
                match recv_st {
                    Ok(s) => {
                        send(
                            &mut socket,
                            &ServerMsg::SiteStats {
                                active_players: s.active_players,
                                logged_in_today: s.logged_in_today,
                            },
                        )
                        .await;
                    }
                    Err(broadcast::error::RecvError::Lagged(_)) => {}
                    Err(broadcast::error::RecvError::Closed) => break,
                }
            }
        }
    }

    if let Some(pid) = registered_player.take() {
        state.presence.leave(&pid);
        broadcast_site_stats(&state);
    }
}

async fn handle_run_action(
    state: &AppState,
    run: &mut Run,
    msg: ClientMsg,
) -> Result<Option<ServerMsg>, String> {
    match msg {
        ClientMsg::BuyCharacter { slot, target } => {
            if run.phase != Phase::Shop {
                return Err("not in shop".into());
            }
            let id = run
                .shop
                .characters
                .get(slot)
                .and_then(|s| s.clone())
                .ok_or("slot empty")?;
            let def = character_def(&id).ok_or("unknown char")?;
            if run.money < def.cost {
                return Err(insufficient_money_msg(def.cost, run.money));
            }
            if run.build.team.len() >= MAX_TEAM {
                return Err("team full".into());
            }
            if target > run.build.team.len() {
                return Err("invalid recruit slot".into());
            }
            run.money -= def.cost;
            run.build.team.insert(
                target,
                TeamMember {
                    def_id: id,
                    hat: None,
                    left_hand: None,
                    right_hand: None,
                },
            );
            run.shop.characters[slot] = None;
            Ok(None)
        }
        ClientMsg::BuyItem { slot, target } => {
            if run.phase != Phase::Shop {
                return Err("not in shop".into());
            }
            let id = run
                .shop
                .items
                .get(slot)
                .and_then(|s| s.clone())
                .ok_or("slot empty")?;
            let def = item_def(&id).ok_or("unknown item")?;
            if run.money < def.cost {
                return Err(insufficient_money_msg(def.cost, run.money));
            }
            let member = run
                .build
                .team
                .get_mut(target)
                .ok_or("no such team member")?;
            match def.slot {
                GearSlot::Hat => {
                    if member.hat.is_some() {
                        return Err("hat slot taken".into());
                    }
                    member.hat = Some(id);
                }
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
        ClientMsg::BuyItemToSlot {
            slot,
            target,
            target_slot,
        } => {
            if run.phase != Phase::Shop {
                return Err("not in shop".into());
            }
            let id = run
                .shop
                .items
                .get(slot)
                .and_then(|s| s.clone())
                .ok_or("slot empty")?;
            let def = item_def(&id).ok_or("unknown item")?;
            if run.money < def.cost {
                return Err(insufficient_money_msg(def.cost, run.money));
            }
            if !slot_accepts(target_slot, def.slot) {
                return Err("wrong item socket".into());
            }
            let member = run
                .build
                .team
                .get_mut(target)
                .ok_or("no such team member")?;
            let dest = member_item_slot_mut(member, target_slot);
            if dest.is_some() {
                return Err("item socket taken".into());
            }
            *dest = Some(id);
            run.money -= def.cost;
            run.shop.items[slot] = None;
            Ok(None)
        }
        ClientMsg::MoveItem {
            from_team,
            from_slot,
            to_team,
            to_slot,
        } => {
            if run.phase != Phase::Shop {
                return Err("not in shop".into());
            }
            if from_team >= run.build.team.len() || to_team >= run.build.team.len() {
                return Err("no such team member".into());
            }
            let item_id = member_item_slot(&run.build.team[from_team], from_slot)
                .as_ref()
                .cloned()
                .ok_or("no item there")?;
            let item_slot = item_def(&item_id).ok_or("unknown item")?.slot;
            if !slot_accepts(to_slot, item_slot) {
                return Err("wrong item socket".into());
            }
            if member_item_slot(&run.build.team[to_team], to_slot).is_some() {
                return Err("item socket taken".into());
            }
            *member_item_slot_mut(&mut run.build.team[from_team], from_slot) = None;
            *member_item_slot_mut(&mut run.build.team[to_team], to_slot) = Some(item_id);
            Ok(None)
        }
        ClientMsg::Sell { team_index } => {
            if run.phase != Phase::Shop {
                return Err("not in shop".into());
            }
            let m = run.build.team.get(team_index).ok_or("no such")?;
            let mut refund = character_def(&m.def_id).map(|d| d.cost).unwrap_or(0);
            for iid in m.item_ids() {
                refund += item_def(iid).map(|d| d.cost).unwrap_or(0);
            }
            run.money += (refund as f32 * SELL_RATIO) as i32;
            run.build.team.remove(team_index);
            Ok(None)
        }
        ClientMsg::SellItem {
            team_index,
            item_slot,
        } => {
            if run.phase != Phase::Shop {
                return Err("not in shop".into());
            }
            let member = run.build.team.get_mut(team_index).ok_or("no such")?;
            let slot = member_item_slot_mut(member, item_slot);
            let item_id = slot.take().ok_or("no item there")?;
            let refund = item_def(&item_id).map(|d| d.cost).unwrap_or(0);
            run.money += (refund as f32 * SELL_RATIO) as i32;
            Ok(None)
        }
        ClientMsg::Reorder { from, to } => {
            if from >= run.build.team.len() || to >= run.build.team.len() {
                return Err("oob".into());
            }
            let m = run.build.team.remove(from);
            run.build.team.insert(to, m);
            Ok(None)
        }
        ClientMsg::Reroll => {
            if run.phase != Phase::Shop {
                return Err("not in shop".into());
            }
            if run.money < REROLL_COST {
                return Err(insufficient_reroll_msg(run.money));
            }
            run.money -= REROLL_COST;
            run.shop = roll_shop();
            Ok(None)
        }
        ClientMsg::Battle => {
            if run.phase != Phase::Shop {
                return Err("not in shop".into());
            }
            if run.build.team.is_empty() {
                return Err("no team".into());
            }
            let player_mmr_before = run.mmr;
            let target_gold = total_earned_gold(run);
            let opponent = state
                .db
                .find_opponent(&run.id, &run.player_id, target_gold)
                .map_err(|e| e.to_string())?;
            let (op_name, op_build_raw, opponent_mmr_before) = match opponent {
                Some((_id, name, b, mmr)) => (name, b, Some(mmr)),
                None => (
                    synthetic_opponent_name(),
                    ai_ladder_build(target_gold.max(50)),
                    None,
                ),
            };
            let res = resolve_battle(&run.build, &op_build_raw);
            if let Some(opponent_mmr) = opponent_mmr_before {
                run.mmr = updated_mmr(player_mmr_before, opponent_mmr, battle_score(res.winner));
            }
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
            if run.losses >= MAX_LOSSES || run.wins >= MAX_WINS {
                run.alive = false;
                run.phase = Phase::GameOver;
            } else {
                run.phase = Phase::Shop;
                run.shop = roll_shop();
            }
            Ok(Some(ServerMsg::Battle {
                run_id: run.id.clone(),
                opponent_name: op_name,
                player_mmr_before,
                opponent_mmr_before,
                events: res.events,
                run: run.clone(),
                winner: res.winner,
                money_after: run.money,
                phase: run.phase,
                wins: run.wins,
                losses: run.losses,
                alive: run.alive,
            }))
        }
        ClientMsg::NextRound => {
            if run.phase == Phase::Battle {
                if run.losses >= MAX_LOSSES || run.wins >= MAX_WINS {
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
            if name.trim().is_empty() {
                return Err("name required".into());
            }
            run.name = name;
            Ok(None)
        }
        ClientMsg::Leaderboard { page, per_page, around_player_id } => {
            leaderboard_msg(state, page, per_page, around_player_id.as_deref()).map(Some)
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

fn leaderboard_msg(
    state: &AppState,
    page: Option<usize>,
    per_page: Option<usize>,
    around_player_id: Option<&str>,
) -> Result<ServerMsg, String> {
    let per_page = per_page
        .unwrap_or(DEFAULT_LEADERBOARD_PAGE_SIZE)
        .clamp(1, MAX_LEADERBOARD_PAGE_SIZE);
    let player_rank = match around_player_id {
        Some(id) if !id.is_empty() => state.db.player_rank(id).map_err(|e| e.to_string())?,
        _ => None,
    };
    let page = match player_rank {
        Some(rank) => ((rank - 1) / per_page) + 1,
        None => page.unwrap_or(1).max(1),
    };
    let (entries, page_count) = state
        .db
        .leaderboard(page, per_page)
        .map_err(|e| e.to_string())?;
    Ok(ServerMsg::Leaderboard {
        entries: entries
            .into_iter()
            .map(|(player_id, name, _streak, wins, mmr)| LbEntry {
                player_id,
                name,
                wins,
                mmr,
            })
            .collect(),
        page: page.min(page_count),
        page_count,
        per_page,
        player_rank,
    })
}

fn total_earned_gold(run: &Run) -> i32 {
    STARTING_MONEY + run.wins * WIN_REWARD + run.losses * LOSE_REWARD
}

fn battle_score(winner: Option<u8>) -> f64 {
    match winner {
        Some(0) => 1.0,
        Some(1) => 0.0,
        _ => 0.5,
    }
}

fn updated_mmr(player_mmr: i32, opponent_mmr: i32, score: f64) -> i32 {
    let expected = 1.0 / (1.0 + 10f64.powf((opponent_mmr - player_mmr) as f64 / 400.0));
    (player_mmr as f64 + MMR_K_FACTOR * (score - expected))
        .round()
        .max(0.0) as i32
}

fn synthetic_opponent_name() -> String {
    const TAGS: &[&str] = &[
        "aesthet1c",
        "vapor",
        "moonbeam",
        "y2k",
        "memehead",
        "cybr",
        "lofi",
        "pix3l",
        "tokr",
        "dolphin",
        "neon",
        "glitch",
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
        GearSlot::Hand => target == ItemSlot::LeftHand || target == ItemSlot::RightHand,
    }
}
