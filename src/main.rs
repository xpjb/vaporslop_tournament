mod auth;
mod db;
mod game;

use auth::RateLimiter;
use axum::{
    extract::{
        ws::{Message, WebSocket, WebSocketUpgrade},
        ConnectInfo, Query, State,
    },
    http::{header, HeaderMap, StatusCode},
    response::IntoResponse,
    routing::{get, post},
    Json, Router,
};
use db::{AttachErr, BattleOutcome, Db};
use game::combat::resolve_v1;
use game::defs::{Defs, DefsVersion};
use game::rng::Rng;
use game::shop::{ai_ladder_build, roll_shop};
use game::types::*;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::{broadcast, Notify};
use tower_http::services::ServeDir;

const AUTH_RATE_LIMIT: usize = 10;
const AUTH_RATE_WINDOW: Duration = Duration::from_secs(60);

#[derive(Clone)]
struct AppState {
    defs: Defs,
    db: Db,
    presence: Arc<Presence>,
    stats_tx: broadcast::Sender<SiteStats>,
    rate: Arc<RateLimiter>,
    /// One slot per `player_id`. When a second WS binds to the same player, the older
    /// holder's `Notify` is fired so it can send `session_replaced` and shut down. Stops
    /// two tabs from interleaving writes against the same `player_state` row.
    active_sessions: Arc<parking_lot::Mutex<HashMap<String, Arc<Notify>>>>,
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
        logged_in_today: state.db.count_players_logged_in_today().unwrap_or(0),
    }
}

fn broadcast_site_stats(state: &AppState) {
    let _ = state.stats_tx.send(site_stats_snapshot(state));
}

/// Bind this socket to `player_id` exactly once: bump presence, log the daily login,
/// and claim the active-session slot (evicting any prior holder).
fn register_socket_player(
    state: &AppState,
    reg: &mut Option<String>,
    evict: &mut Option<Arc<Notify>>,
    player_id: &str,
) {
    if reg.is_none() {
        state.presence.join(player_id);
        if let Err(e) = state.db.touch_player_daily(player_id) {
            tracing::warn!("touch_player_daily: {e}");
        }
        *reg = Some(player_id.to_string());
        *evict = Some(claim_session_slot(state, player_id));
        broadcast_site_stats(state);
    }
}

/// Insert a fresh `Notify` into the active-sessions map for `player_id`. If another socket
/// was already holding the slot, its old `Notify` is fired so it shuts itself down.
fn claim_session_slot(state: &AppState, player_id: &str) -> Arc<Notify> {
    let n = Arc::new(Notify::new());
    let prev = state
        .active_sessions
        .lock()
        .insert(player_id.to_string(), n.clone());
    if let Some(prev) = prev {
        prev.notify_one();
    }
    n
}

/// Remove our entry from the map *only if it's still ours*. (If we were already evicted,
/// the entry now belongs to the new owner; leave it alone.)
fn release_session_slot(state: &AppState, player_id: &str, my_notify: &Arc<Notify>) {
    let mut g = state.active_sessions.lock();
    if let Some(current) = g.get(player_id) {
        if Arc::ptr_eq(current, my_notify) {
            g.remove(player_id);
        }
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
    SetProfile {
        player_id: String,
        name: String,
        selected_avatar: String,
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
        profile_avatars: Vec<ProfileAvatarDef>,
        constants: Constants,
        site_stats: SiteStats,
    },
    SiteStats {
        active_players: u32,
        logged_in_today: u32,
    },
    State {
        run: Run,
        profile: PlayerProfile,
    },
    Profile {
        profile: PlayerProfile,
    },
    Battle {
        run_id: String,
        replay_id: Option<i64>,
        opponent_name: String,
        player_avatar: String,
        opponent_avatar: String,
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
        /// Seed that drove `resolve_v1`. Persisted on the replay row.
        combat_seed: u32,
        /// Pool id of the enemy snapshot (`None` for synthetic AI). Persisted on the replay row.
        enemy_opponent_id: Option<i64>,
    },
    Leaderboard {
        entries: Vec<LbEntry>,
        page: usize,
        page_count: usize,
        per_page: usize,
        player_rank: Option<usize>,
        player_mmr: Option<i32>,
    },
    Error {
        message: String,
    },
    AuthRequired,
    /// Another socket has taken over this player's slot. Client should stop sending and
    /// surface a "this run is being played in another tab" notice.
    SessionReplaced,
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
    avatar: String,
    wins: i32,
    mmr: i32,
}

#[derive(Debug, Serialize)]
struct ReplayPayload {
    replay_id: i64,
    player_name: String,
    opponent_name: String,
    player_avatar: String,
    opponent_avatar: String,
    player_mmr_before: i32,
    opponent_mmr_before: i32,
    events: Vec<game::combat::CombatEvent>,
    winner: Option<u8>,
    created_at: i64,
    version_mismatch: bool,
}

const DEFAULT_LEADERBOARD_PAGE_SIZE: usize = 10;
const MAX_LEADERBOARD_PAGE_SIZE: usize = 50;
const DEFAULT_PROFILE_AVATAR: &str = "meme_man";

fn profile_avatar_defs() -> Vec<ProfileAvatarDef> {
    vec![
        profile_avatar("meme_man", "Meme Man", "Meme_Man.webp", 0, 0),
        profile_avatar("orang", "Orang", "orang.webp", 3, 0),
        profile_avatar("vegetal", "Vegetal", "vegetal.webp", 6, 0),
        profile_avatar("dark_vegetal", "Dark Vegetal", "dark_vegetal.webp", 9, 0),
        profile_avatar("picardia", "Picardia", "picardia.webp", 12, 0),
        profile_avatar("lemen", "Lemen", "Lemen_man.webp", 15, 0),
        profile_avatar(
            "azul_picardia",
            "Azul Picardia",
            "azul_picardia.webp",
            18,
            0,
        ),
        profile_avatar("gren", "Gren", "gren.webp", 21, 0),
        profile_avatar("isoceles", "Isoceles", "Isosceles.webp", 24, 0),
        profile_avatar(
            "omniscronchulon",
            "Omniscronchulon",
            "omniscronchulon.webp",
            27,
            0,
        ),
        profile_avatar("elephoont", "Elephoont", "elephoont.webp", MAX_WINS, 1),
        profile_avatar("noggin", "Noggin", "Noggin.webp", 0, 10),
        profile_avatar(
            "pickle_rick",
            "Hotdog Harry",
            "hotdog_harry.png",
            0,
            100,
        ),
    ]
}

fn profile_avatar(
    id: &str,
    name: &str,
    sprite: &str,
    required_wins: i32,
    required_ultimate_victories: i32,
) -> ProfileAvatarDef {
    ProfileAvatarDef {
        id: id.into(),
        name: name.into(),
        sprite: sprite.into(),
        required_wins,
        required_ultimate_victories,
    }
}

fn avatar_is_unlocked(profile: &PlayerProfile, avatar: &ProfileAvatarDef) -> bool {
    profile.best_wins >= avatar.required_wins
        && profile.ultimate_victories >= avatar.required_ultimate_victories
}

fn sanitize_profile_avatar(profile: &PlayerProfile, requested: &str) -> String {
    profile_avatar_defs()
        .into_iter()
        .find(|avatar| avatar.id == requested && avatar_is_unlocked(profile, avatar))
        .map(|avatar| avatar.id)
        .unwrap_or_else(|| DEFAULT_PROFILE_AVATAR.to_string())
}

fn sanitize_stored_profile(mut profile: PlayerProfile) -> PlayerProfile {
    let selected = sanitize_profile_avatar(&profile, &profile.selected_avatar);
    profile.selected_avatar = selected;
    profile
}

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
    let defs = Defs::load();
    let db = Db::open("vaporslop.sqlite", &defs.current_table())?;
    let (stats_tx, _) = broadcast::channel::<SiteStats>(64);
    let state = Arc::new(AppState {
        defs,
        db,
        presence: Arc::new(Presence::default()),
        stats_tx,
        rate: RateLimiter::new(),
        active_sessions: Arc::new(parking_lot::Mutex::new(HashMap::new())),
    });

    let app = Router::new()
        .route("/ws", get(ws_handler))
        .route("/api/whoami", get(whoami_handler))
        .route("/api/replay", get(replay_handler))
        .route("/api/replays", get(replays_list_handler))
        .route("/api/register", post(register_handler))
        .route("/api/login", post(login_handler))
        .route("/api/logout", post(logout_handler))
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
    axum::serve(
        listener,
        app.into_make_service_with_connect_info::<SocketAddr>(),
    )
    .await?;
    Ok(())
}

async fn ws_handler(
    ws: WebSocketUpgrade,
    headers: HeaderMap,
    State(state): State<Arc<AppState>>,
) -> impl IntoResponse {
    let session_pid = auth::parse_session_cookie(&headers).and_then(|token| {
        state
            .db
            .lookup_session(&token, auth::SHORT_SESSION_TTL_SECS)
            .ok()
            .flatten()
    });
    ws.on_upgrade(move |socket| handle_socket(socket, state, session_pid))
}

async fn send(socket: &mut WebSocket, msg: &ServerMsg) {
    if let Ok(s) = serde_json::to_string(msg) {
        let _ = socket.send(Message::Text(s)).await;
    }
}

/// Resolve the effective `player_id` for a message. If a session is active, override the
/// body's id with the session's. Otherwise, refuse if the body's id is registered.
fn resolve_player_id(
    db: &Db,
    session_pid: Option<&str>,
    body_pid: &str,
) -> Result<String, ()> {
    if let Some(s) = session_pid {
        return Ok(s.to_string());
    }
    let cleaned = clean_player_id(body_pid);
    match db.username_for_player(&cleaned) {
        Ok(Some(_)) => Err(()),
        _ => Ok(cleaned),
    }
}

async fn handle_socket(mut socket: WebSocket, state: Arc<AppState>, session_pid: Option<String>) {
    let mut registered_player: Option<String> = None;
    let mut evict_notify: Option<Arc<Notify>> = None;
    let mut rx = state.stats_tx.subscribe();

    let live_defs = state.defs.current_table();
    send(
        &mut socket,
        &ServerMsg::Defs {
            characters: live_defs.sorted_characters(),
            items: live_defs.sorted_items(),
            profile_avatars: profile_avatar_defs(),
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
        // Per-iteration future that resolves when this socket's slot is evicted by a newer
        // socket binding to the same player_id. Cloned each loop so it doesn't borrow
        // `evict_notify` (other arms below need to mutate it). An unawaited Notified does
        // not consume the permit, so a notification fired during another arm's iteration
        // is picked up on the next loop.
        let evict_handle = evict_notify.clone();
        let evict_fut = async move {
            if let Some(n) = evict_handle {
                n.notified().await;
            } else {
                std::future::pending::<()>().await;
            }
        };
        tokio::pin!(evict_fut);

        tokio::select! {
            biased;
            _ = &mut evict_fut => {
                send(&mut socket, &ServerMsg::SessionReplaced).await;
                break;
            }
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
                        let player_id = match resolve_player_id(&state.db, session_pid.as_deref(), &player_id) {
                            Ok(p) => p,
                            Err(()) => {
                                send(&mut socket, &ServerMsg::AuthRequired).await;
                                continue;
                            }
                        };
                        let profile = match state.db.ensure_player_profile(&player_id, &name) {
                            Ok(profile) => sanitize_stored_profile(profile),
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
                        let mut rng = Rng::new_random();
                        let run = Run {
                            id: uuid::Uuid::new_v4().to_string(),
                            player_id,
                            name: profile.name.clone(),
                            money: STARTING_MONEY,
                            wins: 0,
                            losses: 0,
                            streak: 0,
                            alive: true,
                            best_streak: 0,
                            mmr,
                            build: Build::default(),
                            shop: roll_shop(&state.defs.current_table(), &mut rng),
                            phase: Phase::Shop,
                        };
                        register_socket_player(&state, &mut registered_player, &mut evict_notify, &run.player_id);
                        if let Err(e) = state.db.upsert_player_state(&run) {
                            send(&mut socket, &ServerMsg::Error { message: e.to_string() }).await;
                            continue;
                        }
                        send(&mut socket, &ServerMsg::State { run, profile }).await;
                    }
                    ClientMsg::Resume { player_id } => {
                        let player_id = match resolve_player_id(&state.db, session_pid.as_deref(), &player_id) {
                            Ok(p) => p,
                            Err(()) => {
                                send(&mut socket, &ServerMsg::AuthRequired).await;
                                continue;
                            }
                        };
                        register_socket_player(&state, &mut registered_player, &mut evict_notify, &player_id);
                        match state.db.load_player_state(&player_id) {
                            Ok(Some(run)) => {
                                let mut profile = match state.db.ensure_player_profile(&player_id, &run.name) {
                                    Ok(profile) => sanitize_stored_profile(profile),
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
                                if profile.name == "anon" && run.name.trim() != "anon" && !run.name.trim().is_empty() {
                                    profile = match state.db.backfill_profile_name(&player_id, &run.name) {
                                        Ok(profile) => sanitize_stored_profile(profile),
                                        Err(e) => {
                                            send(&mut socket, &ServerMsg::Error { message: e.to_string() }).await;
                                            continue;
                                        }
                                    };
                                }
                                send(&mut socket, &ServerMsg::Profile { profile: profile.clone() }).await;
                                send(&mut socket, &ServerMsg::State { run, profile }).await;
                            }
                            Ok(None) => {
                                let profile = match state.db.ensure_player_profile(&player_id, "anon") {
                                    Ok(profile) => sanitize_stored_profile(profile),
                                    Err(e) => {
                                        send(&mut socket, &ServerMsg::Error { message: e.to_string() }).await;
                                        continue;
                                    }
                                };
                                send(&mut socket, &ServerMsg::Profile { profile }).await;
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
                    ClientMsg::SetProfile { player_id, name, selected_avatar } => {
                        let player_id = match resolve_player_id(&state.db, session_pid.as_deref(), &player_id) {
                            Ok(p) => p,
                            Err(()) => {
                                send(&mut socket, &ServerMsg::AuthRequired).await;
                                continue;
                            }
                        };
                        register_socket_player(&state, &mut registered_player, &mut evict_notify, &player_id);
                        let current_profile = match state.db.ensure_player_profile(&player_id, &name) {
                            Ok(profile) => sanitize_stored_profile(profile),
                            Err(e) => {
                                send(&mut socket, &ServerMsg::Error { message: e.to_string() }).await;
                                continue;
                            }
                        };
                        let avatar = sanitize_profile_avatar(&current_profile, &selected_avatar);
                        let profile = match state.db.update_player_profile(&player_id, &name, &avatar) {
                            Ok(profile) => sanitize_stored_profile(profile),
                            Err(e) => {
                                send(&mut socket, &ServerMsg::Error { message: e.to_string() }).await;
                                continue;
                            }
                        };
                        send(&mut socket, &ServerMsg::Profile { profile: profile.clone() }).await;
                        // `update_player_profile` already renamed the player_state row in SQL;
                        // reload to pick up the new name and echo State back to the client.
                        if let Ok(Some(run)) = state.db.load_player_state(&player_id) {
                            send(&mut socket, &ServerMsg::State { run, profile }).await;
                        }
                    }
                    other => {
                        let player_id = match &registered_player {
                            Some(p) => p.clone(),
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
                        let mut run = match state.db.load_player_state(&player_id) {
                            Ok(Some(r)) => r,
                            Ok(None) => {
                                send(
                                    &mut socket,
                                    &ServerMsg::Error {
                                        message: "run gone".into(),
                                    },
                                )
                                .await;
                                continue;
                            }
                            Err(e) => {
                                send(&mut socket, &ServerMsg::Error { message: e.to_string() }).await;
                                continue;
                            }
                        };
                        let is_battle = matches!(&other, ClientMsg::Battle);
                        let defs_table = state.defs.current_table();
                        let result = handle_run_action(&state, &defs_table, &mut run, other).await;
                        match result {
                            Ok(mut extra) => {
                                let save_result = if is_battle {
                                    let (update_mmr, outcome, combat_seed, enemy_opponent_id, player_mmr_before) =
                                        match &extra {
                                            Some(ServerMsg::Battle {
                                                winner,
                                                opponent_mmr_before,
                                                combat_seed,
                                                enemy_opponent_id,
                                                player_mmr_before,
                                                ..
                                            }) => (
                                                opponent_mmr_before.is_some(),
                                                battle_outcome(*winner),
                                                *combat_seed,
                                                *enemy_opponent_id,
                                                *player_mmr_before,
                                            ),
                                            _ => (false, BattleOutcome::Draw, 0u32, None, run.mmr),
                                        };
                                    let completed_ultimate_victory =
                                        run.phase == Phase::GameOver && run.wins >= MAX_WINS;
                                    state.db.record_battle_and_save_state(
                                        &run,
                                        outcome,
                                        update_mmr,
                                        completed_ultimate_victory,
                                        enemy_opponent_id,
                                        combat_seed,
                                        player_mmr_before,
                                        &defs_table,
                                    )
                                } else {
                                    state.db.upsert_player_state(&run).map(|_| None)
                                };
                                let replay_id = match save_result {
                                    Ok(id) => id,
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
                                if let Some(ServerMsg::Battle { replay_id: slot, .. }) =
                                    extra.as_mut()
                                {
                                    *slot = replay_id;
                                }
                                if is_battle {
                                    if let Ok(profile) = state.db.ensure_player_profile(&run.player_id, &run.name) {
                                        send(
                                            &mut socket,
                                            &ServerMsg::Profile {
                                                profile: sanitize_stored_profile(profile),
                                            },
                                        )
                                        .await;
                                    }
                                }
                                if let Some(extra) = extra {
                                    send(&mut socket, &extra).await;
                                    if is_battle {
                                        continue;
                                    }
                                }
                                let profile = state
                                    .db
                                    .ensure_player_profile(&run.player_id, &run.name)
                                    .map(sanitize_stored_profile)
                                    .unwrap_or_else(|_| PlayerProfile {
                                        player_id: run.player_id.clone(),
                                        name: run.name.clone(),
                                        selected_avatar: DEFAULT_PROFILE_AVATAR.to_string(),
                                        best_wins: 0,
                                        ultimate_victories: 0,
                                    });
                                send(&mut socket, &ServerMsg::State { run, profile }).await;
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
        if let Some(notify) = evict_notify.as_ref() {
            release_session_slot(&state, &pid, notify);
        }
        state.presence.leave(&pid);
        broadcast_site_stats(&state);
    }
}

async fn handle_run_action(
    state: &AppState,
    defs: &game::defs::DefsTable,
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
            let def = defs.unit(&id).ok_or("unknown char")?;
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
                    hand_3: None,
                    hand_4: None,
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
            let def = defs.item(&id).ok_or("unknown item")?;
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
                    let hand_count = defs.unit(&member.def_id)
                        .map(|d| d.hand_slots())
                        .unwrap_or(2) as usize;
                    let target_slot = ItemSlot::HAND_SLOTS
                        .iter()
                        .take(hand_count)
                        .find(|s| member.hand_slot(**s).is_none())
                        .copied();
                    match target_slot {
                        Some(s) => *member.hand_slot_mut(s) = Some(id),
                        None => return Err("all hands full".into()),
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
            let def = defs.item(&id).ok_or("unknown item")?;
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
            if !member_has_slot(defs, member, target_slot) {
                return Err("character lacks that slot".into());
            }
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
            let item_slot = defs.item(&item_id).ok_or("unknown item")?.slot;
            if !slot_accepts(to_slot, item_slot) {
                return Err("wrong item socket".into());
            }
            if !member_has_slot(defs, &run.build.team[to_team], to_slot) {
                return Err("character lacks that slot".into());
            }
            if from_team == to_team && from_slot == to_slot {
                return Ok(None);
            }
            let swapped_item_id = member_item_slot(&run.build.team[to_team], to_slot)
                .as_ref()
                .cloned();
            if let Some(swapped_item_id) = swapped_item_id.as_ref() {
                let swapped_slot = defs.item(swapped_item_id).ok_or("unknown item")?.slot;
                if !slot_accepts(from_slot, swapped_slot) {
                    return Err("wrong item socket".into());
                }
            }
            *member_item_slot_mut(&mut run.build.team[from_team], from_slot) = swapped_item_id;
            *member_item_slot_mut(&mut run.build.team[to_team], to_slot) = Some(item_id);
            Ok(None)
        }
        ClientMsg::Sell { team_index } => {
            if run.phase != Phase::Shop {
                return Err("not in shop".into());
            }
            let m = run.build.team.get(team_index).ok_or("no such")?;
            let mut refund = defs.unit(&m.def_id).map(|d| d.cost).unwrap_or(0);
            for iid in m.item_ids() {
                refund += defs.item(iid).map(|d| d.cost).unwrap_or(0);
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
            let refund = defs.item(&item_id).map(|d| d.cost).unwrap_or(0);
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
            run.shop = roll_shop(defs, &mut Rng::new_random());
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
            // `mm_rng` drives matchmaking + (when fallback) bot generation. `combat_seed`
            // is recorded on the replay row so the fight is byte-for-byte reproducible
            // independently of which opponent was picked.
            let mut mm_rng = Rng::new_random();
            let opponent = state
                .db
                .find_opponent(&run.player_id, target_gold, &mut mm_rng)
                .map_err(|e| e.to_string())?;
            let player_avatar = state
                .db
                .profile_avatar(&run.player_id)
                .ok()
                .flatten()
                .unwrap_or_else(|| DEFAULT_PROFILE_AVATAR.to_string());
            let (op_name, op_build_raw, opponent_mmr_before, opponent_avatar, enemy_opponent_id) =
                match opponent {
                    Some((opponent_id, opponent_player_id, b, mmr)) => {
                        let name = state
                            .db
                            .profile_name(&opponent_player_id)
                            .ok()
                            .flatten()
                            .unwrap_or_else(|| "anon".to_string());
                        let avatar = state
                            .db
                            .profile_avatar(&opponent_player_id)
                            .ok()
                            .flatten()
                            .unwrap_or_else(|| DEFAULT_PROFILE_AVATAR.to_string());
                        (name, b, Some(mmr), avatar, Some(opponent_id))
                    }
                    None => (
                        synthetic_opponent_name(&mut mm_rng),
                        ai_ladder_build(defs, target_gold.max(50), &mut mm_rng),
                        None,
                        DEFAULT_PROFILE_AVATAR.to_string(),
                        None,
                    ),
                };
            let combat_seed: u32 = Rng::new_random().next_u32();
            let player_team = defs
                .resolve(&run.build)
                .ok_or_else(|| "unable to materialize team".to_string())?;
            let enemy_team = defs
                .resolve(&op_build_raw)
                .ok_or_else(|| "opponent build invalid".to_string())?;
            let mut combat_rng = Rng::new(combat_seed);
            let res = resolve_v1(defs, &player_team, &enemy_team, &mut combat_rng);
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
                run.shop = roll_shop(defs, &mut mm_rng);
            }
            Ok(Some(ServerMsg::Battle {
                run_id: run.id.clone(),
                replay_id: None,
                opponent_name: op_name,
                player_avatar,
                opponent_avatar,
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
                combat_seed,
                enemy_opponent_id,
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
        ClientMsg::Leaderboard {
            page,
            per_page,
            around_player_id,
        } => leaderboard_msg(state, page, per_page, around_player_id.as_deref()).map(Some),
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
    let player_info = match around_player_id {
        Some(id) if !id.is_empty() => state.db.player_rank(id).map_err(|e| e.to_string())?,
        _ => None,
    };
    let player_rank = player_info.map(|(rank, _)| rank);
    let player_mmr = player_info.map(|(_, mmr)| mmr);
    let page = match (page, player_rank) {
        (Some(p), _) => p.max(1),
        (None, Some(rank)) => ((rank - 1) / per_page) + 1,
        (None, None) => 1,
    };
    let (entries, page_count) = state
        .db
        .leaderboard(page, per_page)
        .map_err(|e| e.to_string())?;
    Ok(ServerMsg::Leaderboard {
        entries: entries
            .into_iter()
            .map(|(player_id, name, wins, mmr, avatar)| LbEntry {
                player_id,
                name,
                avatar,
                wins,
                mmr,
            })
            .collect(),
        page: page.min(page_count),
        page_count,
        per_page,
        player_rank,
        player_mmr,
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

fn battle_outcome(winner: Option<u8>) -> BattleOutcome {
    match winner {
        Some(0) => BattleOutcome::Win,
        Some(1) => BattleOutcome::Loss,
        _ => BattleOutcome::Draw,
    }
}

fn updated_mmr(player_mmr: i32, opponent_mmr: i32, score: f64) -> i32 {
    let expected = 1.0 / (1.0 + 10f64.powf((opponent_mmr - player_mmr) as f64 / 400.0));
    (player_mmr as f64 + MMR_K_FACTOR * (score - expected))
        .round()
        .max(0.0) as i32
}

fn synthetic_opponent_name(rng: &mut Rng) -> String {
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
    let tag = rng.choice(TAGS).copied().unwrap();
    let n = (rng.next_u32() % 999) + 1;
    format!("{}_bot_{:03}", tag, n)
}

fn member_item_slot(member: &TeamMember, slot: ItemSlot) -> &Option<String> {
    member.hand_slot(slot)
}

fn member_item_slot_mut(member: &mut TeamMember, slot: ItemSlot) -> &mut Option<String> {
    member.hand_slot_mut(slot)
}

fn slot_accepts(target: ItemSlot, item: GearSlot) -> bool {
    match item {
        GearSlot::Hat => target == ItemSlot::Hat,
        GearSlot::Hand => matches!(
            target,
            ItemSlot::LeftHand | ItemSlot::RightHand | ItemSlot::Hand3 | ItemSlot::Hand4
        ),
    }
}

fn member_has_slot(defs: &game::defs::DefsTable, member: &TeamMember, slot: ItemSlot) -> bool {
    let hand_count = defs.unit(&member.def_id)
        .map(|d| d.hand_slots())
        .unwrap_or(2);
    match slot {
        ItemSlot::Hat | ItemSlot::LeftHand | ItemSlot::RightHand => true,
        ItemSlot::Hand3 => hand_count >= 3,
        ItemSlot::Hand4 => hand_count >= 4,
    }
}

#[derive(Debug, Deserialize)]
struct WhoamiQuery {
    player_id: Option<String>,
}

#[derive(Debug, Serialize)]
struct WhoamiResponse {
    player_id: Option<String>,
    username: Option<String>,
    display_name: Option<String>,
    avatar: Option<String>,
    has_account: bool,
    signed_in: bool,
}

async fn whoami_handler(
    headers: HeaderMap,
    State(state): State<Arc<AppState>>,
    Query(q): Query<WhoamiQuery>,
) -> impl IntoResponse {
    let (signed_in_pid, signed_in) = if let Some(token) = auth::parse_session_cookie(&headers) {
        match state.db.lookup_session(&token, auth::SHORT_SESSION_TTL_SECS) {
            Ok(Some(pid)) => (Some(pid), true),
            _ => (None, false),
        }
    } else {
        (None, false)
    };

    let pid: Option<String> = signed_in_pid.or_else(|| {
        q.player_id
            .as_deref()
            .map(clean_player_id)
            .filter(|s| !s.is_empty())
    });

    let (username, display_name, avatar) = match pid.as_deref() {
        Some(p) => {
            let username = state.db.username_for_player(p).ok().flatten();
            let prof = state.db.load_profile(p).ok().flatten();
            let display_name = prof.as_ref().map(|p| p.name.clone());
            let avatar = prof.map(|p| sanitize_stored_profile(p).selected_avatar);
            (username, display_name, avatar)
        }
        None => (None, None, None),
    };

    Json(WhoamiResponse {
        player_id: pid,
        has_account: username.is_some(),
        username,
        display_name,
        avatar,
        signed_in,
    })
    .into_response()
}

#[derive(Debug, Deserialize)]
struct ReplayQuery {
    battle_id: i64,
}

async fn replay_handler(
    State(state): State<Arc<AppState>>,
    Query(q): Query<ReplayQuery>,
) -> axum::response::Response {
    let replay = match state.db.replay_record(q.battle_id) {
        Ok(Some(replay)) => replay,
        Ok(None) => {
            return (
                StatusCode::NOT_FOUND,
                Json(serde_json::json!({ "error": "replay_not_found" })),
            )
                .into_response()
        }
        Err(e) => {
            tracing::error!("replay_record: {e}");
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(serde_json::json!({ "error": "server_error" })),
            )
                .into_response();
        }
    };
    let player_name = state
        .db
        .profile_name(&replay.player_id)
        .ok()
        .flatten()
        .unwrap_or_else(|| "anon".to_string());
    let opponent_name = state
        .db
        .profile_name(&replay.enemy_player_id)
        .ok()
        .flatten()
        .unwrap_or_else(|| "anon".to_string());
    let player_avatar = state
        .db
        .profile_avatar(&replay.player_id)
        .ok()
        .flatten()
        .unwrap_or_else(|| DEFAULT_PROFILE_AVATAR.to_string());
    let opponent_avatar = state
        .db
        .profile_avatar(&replay.enemy_player_id)
        .ok()
        .flatten()
        .unwrap_or_else(|| DEFAULT_PROFILE_AVATAR.to_string());
    let recorded_winner = match replay.outcome {
        BattleOutcome::Win => Some(0),
        BattleOutcome::Loss => Some(1),
        BattleOutcome::Draw => None,
    };
    let version_mismatch = replay.version_hash != state.defs.current_version().0;
    let replay_ver = DefsVersion(replay.version_hash);
    let (events, winner) = match state.defs.table_at(replay_ver) {
        None => (vec![], recorded_winner),
        Some(table) => match (
            table.resolve(&replay.player_build),
            table.resolve(&replay.enemy_build),
        ) {
            (Some(player_team), Some(enemy_team)) => {
                let mut combat_rng = Rng::new(replay.combat_seed);
                let battle =
                    resolve_v1(&table, &player_team, &enemy_team, &mut combat_rng);
                (battle.events, battle.winner)
            }
            _ => (vec![], recorded_winner),
        },
    };
    Json(ReplayPayload {
        replay_id: replay.id,
        player_name,
        opponent_name,
        player_avatar,
        opponent_avatar,
        player_mmr_before: replay.player_mmr_before,
        opponent_mmr_before: replay.enemy_mmr_before,
        events,
        winner,
        created_at: replay.created_at,
        version_mismatch,
    })
    .into_response()
}

const DEFAULT_REPLAYS_PAGE_SIZE: usize = 25;
const MAX_REPLAYS_PAGE_SIZE: usize = 100;

#[derive(Debug, Deserialize)]
struct ReplaysListQuery {
    scope: Option<String>,
    player_id: Option<String>,
    page: Option<usize>,
    per_page: Option<usize>,
}

#[derive(Debug, Serialize)]
struct ReplayListEntry {
    id: i64,
    player_id: String,
    player_name: String,
    player_avatar: String,
    player_mmr_before: i32,
    enemy_player_id: String,
    enemy_name: String,
    enemy_avatar: String,
    enemy_mmr_before: i32,
    outcome: &'static str,
    created_at: i64,
}

#[derive(Debug, Serialize)]
struct ReplaysListPayload {
    entries: Vec<ReplayListEntry>,
    page: usize,
    page_count: usize,
    per_page: usize,
    scope: &'static str,
}

async fn replays_list_handler(
    State(state): State<Arc<AppState>>,
    Query(q): Query<ReplaysListQuery>,
) -> axum::response::Response {
    let scope = q.scope.as_deref().unwrap_or("all");
    let per_page = q
        .per_page
        .unwrap_or(DEFAULT_REPLAYS_PAGE_SIZE)
        .clamp(1, MAX_REPLAYS_PAGE_SIZE);
    let page = q.page.unwrap_or(1).max(1);
    let (filter, scope_label): (Option<&str>, &'static str) = match scope {
        "mine" => {
            let Some(pid) = q.player_id.as_deref().filter(|s| !s.is_empty()) else {
                return (
                    StatusCode::BAD_REQUEST,
                    Json(serde_json::json!({ "error": "player_id_required" })),
                )
                    .into_response();
            };
            (Some(pid), "mine")
        }
        _ => (None, "all"),
    };
    let (rows, page_count) = match state.db.replays_list(filter, page, per_page) {
        Ok(out) => out,
        Err(e) => {
            tracing::error!("replays_list: {e}");
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(serde_json::json!({ "error": "server_error" })),
            )
                .into_response();
        }
    };
    let entries: Vec<ReplayListEntry> = rows
        .into_iter()
        .map(|r| ReplayListEntry {
            id: r.id,
            player_id: r.player_id,
            player_name: r.player_name,
            player_avatar: r.player_avatar,
            player_mmr_before: r.player_mmr_before,
            enemy_player_id: r.enemy_player_id,
            enemy_name: r.enemy_name,
            enemy_avatar: r.enemy_avatar,
            enemy_mmr_before: r.enemy_mmr_before,
            outcome: r.outcome.as_str(),
            created_at: r.created_at,
        })
        .collect();
    Json(ReplaysListPayload {
        entries,
        page: page.min(page_count),
        page_count,
        per_page,
        scope: scope_label,
    })
    .into_response()
}

#[derive(Debug, Deserialize)]
struct RegisterRequest {
    player_id: String,
    username: String,
    password: String,
}

#[derive(Debug, Serialize)]
struct AuthErrorBody {
    error: String,
}

fn auth_err(code: StatusCode, msg: &str) -> axum::response::Response {
    (
        code,
        Json(AuthErrorBody {
            error: msg.to_string(),
        }),
    )
        .into_response()
}

async fn register_handler(
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
    State(state): State<Arc<AppState>>,
    Json(req): Json<RegisterRequest>,
) -> axum::response::Response {
    let ip = auth::client_ip(&headers, peer.ip());
    if !state.rate.allow(ip, AUTH_RATE_LIMIT, AUTH_RATE_WINDOW) {
        return auth_err(StatusCode::TOO_MANY_REQUESTS, "rate_limited");
    }
    let username = match auth::validate_username(&req.username) {
        Ok(u) => u,
        Err(e) => return auth_err(StatusCode::BAD_REQUEST, e),
    };
    if let Err(e) = auth::validate_password(&req.password) {
        return auth_err(StatusCode::BAD_REQUEST, e);
    }
    let player_id = clean_player_id(&req.player_id);
    if player_id.is_empty() {
        return auth_err(StatusCode::BAD_REQUEST, "player_id_required");
    }
    let hash = match auth::hash_password(&req.password) {
        Ok(h) => h,
        Err(e) => {
            tracing::error!("argon2: {e}");
            return auth_err(StatusCode::INTERNAL_SERVER_ERROR, "server_error");
        }
    };
    match state.db.attach_credentials(&player_id, &username, &hash) {
        Ok(()) => {}
        Err(AttachErr::AlreadyRegistered) => {
            return auth_err(StatusCode::CONFLICT, "already_registered")
        }
        Err(AttachErr::UsernameTaken) => {
            return auth_err(StatusCode::CONFLICT, "username_taken")
        }
        Err(AttachErr::Db(e)) => {
            tracing::error!("attach_credentials: {e}");
            return auth_err(StatusCode::INTERNAL_SERVER_ERROR, "server_error");
        }
    }
    let token = match state
        .db
        .create_session(&player_id, auth::SHORT_SESSION_TTL_SECS)
    {
        Ok(t) => t,
        Err(e) => {
            tracing::error!("create_session: {e}");
            return auth_err(StatusCode::INTERNAL_SERVER_ERROR, "server_error");
        }
    };
    let cookie = auth::set_session_cookie(&token, auth::SHORT_SESSION_TTL_SECS, &headers);
    (
        StatusCode::OK,
        [(header::SET_COOKIE, cookie)],
        Json(serde_json::json!({ "ok": true, "player_id": player_id })),
    )
        .into_response()
}

#[derive(Debug, Deserialize)]
struct LoginRequest {
    username: String,
    password: String,
    #[serde(default)]
    stay: bool,
}

async fn login_handler(
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
    State(state): State<Arc<AppState>>,
    Json(req): Json<LoginRequest>,
) -> axum::response::Response {
    let ip = auth::client_ip(&headers, peer.ip());
    if !state.rate.allow(ip, AUTH_RATE_LIMIT, AUTH_RATE_WINDOW) {
        return auth_err(StatusCode::TOO_MANY_REQUESTS, "rate_limited");
    }
    let username = req.username.trim().to_string();
    if username.is_empty() || req.password.is_empty() {
        return auth_err(StatusCode::BAD_REQUEST, "invalid_credentials");
    }
    let account = match state.db.find_account(&username) {
        Ok(a) => a,
        Err(e) => {
            tracing::error!("find_account: {e}");
            return auth_err(StatusCode::INTERNAL_SERVER_ERROR, "server_error");
        }
    };
    let (player_id, hash) = match account {
        Some(a) => a,
        None => return auth_err(StatusCode::UNAUTHORIZED, "invalid_credentials"),
    };
    let password = req.password.clone();
    let hash_clone = hash.clone();
    let verified = tokio::task::spawn_blocking(move || auth::verify_password(&password, &hash_clone))
        .await
        .unwrap_or(false);
    if !verified {
        return auth_err(StatusCode::UNAUTHORIZED, "invalid_credentials");
    }
    let ttl = if req.stay {
        auth::LONG_SESSION_TTL_SECS
    } else {
        auth::SHORT_SESSION_TTL_SECS
    };
    let token = match state.db.create_session(&player_id, ttl) {
        Ok(t) => t,
        Err(e) => {
            tracing::error!("create_session: {e}");
            return auth_err(StatusCode::INTERNAL_SERVER_ERROR, "server_error");
        }
    };
    let cookie = auth::set_session_cookie(&token, ttl, &headers);
    (
        StatusCode::OK,
        [(header::SET_COOKIE, cookie)],
        Json(serde_json::json!({ "ok": true, "player_id": player_id })),
    )
        .into_response()
}

async fn logout_handler(
    headers: HeaderMap,
    State(state): State<Arc<AppState>>,
) -> axum::response::Response {
    if let Some(token) = auth::parse_session_cookie(&headers) {
        let _ = state.db.delete_session(&token);
    }
    let cookie = auth::clear_session_cookie(&headers);
    (
        StatusCode::OK,
        [(header::SET_COOKIE, cookie)],
        Json(serde_json::json!({ "ok": true })),
    )
        .into_response()
}
