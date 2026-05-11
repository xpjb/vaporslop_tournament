import { computePosition, autoUpdate, offset, flip, shift } from "https://cdn.jsdelivr.net/npm/@floating-ui/dom/+esm";
import { playBattle } from "./render.js";
import { mountBasePath } from "./path-base.js";

const BASE_PATH = mountBasePath(import.meta);
function assetHref(sprite) {
  return `${BASE_PATH}/assets/${sprite}`;
}

const $ = (sel) => document.querySelector(sel);
const $$ = (sel) => Array.from(document.querySelectorAll(sel));

const PLAYER_ID_KEY = "playerUuid";
const NICKNAME_KEY = "playerNickname";
const BGM_VOLUME_KEY = "bgmVolume";
const BGM_MUTED_KEY = "bgmMuted";
const LB_PAGE_SIZE = 25;
const BGM_CROSSFADE_MS = 1000;
const DEFAULT_AVATAR_ID = "meme_man";

function requestedReplayIdFromUrl() {
  const raw = new URLSearchParams(window.location.search).get("replay");
  const replayId = Number.parseInt(raw || "", 10);
  return Number.isInteger(replayId) && replayId > 0 ? replayId : null;
}

const state = {
  ws: null,
  defs: { characters: [], items: [] },
  profileAvatars: [],
  consts: {},
  run: null,
  profile: {
    player_id: "",
    name: localStorage.getItem(NICKNAME_KEY) || "anon",
    selected_avatar: DEFAULT_AVATAR_ID,
    best_wins: 0,
    ultimate_victories: 0,
  },
  profileDraftAvatar: DEFAULT_AVATAR_ID,
  pendingItem: null, // shop item slot waiting for team target
  lastBattle: null,
  battleAnimating: false,
  autoResumePending: false,
  evicted: false, // server told us another tab took over; stay disconnected
  playerId: getOrCreatePlayerId(),
  nickname: localStorage.getItem(NICKNAME_KEY) || "anon",
  auth: {
    username: null,
    displayName: null,
    avatar: null,
    hasAccount: false,
    signedIn: false,
    edgeCase: false, // registered uuid on this device, no active session
  },
  bgmAudio: null,
  bgmAltAudio: null,
  bgmVolume: getStoredBgmVolume(),
  bgmMuted: localStorage.getItem(BGM_MUTED_KEY) === "1",
  bgmAltActive: false,
  bgmAltMix: 0,
  bgmFadeRaf: null,
  lb: {
    entries: [], // contiguous list of {rank, player_id, name, mmr, wins}
    minPage: null,
    maxPage: null,
    pageCount: 1,
    perPage: LB_PAGE_SIZE,
    playerRank: null,
    playerMmr: null,
    loading: false,
    pendingScroll: null, // "top" | "rank:<n>" | null
    open: false,
  },
  sharedReplay: {
    requestedId: requestedReplayIdFromUrl(),
    activeId: null,
    loading: false,
  },
};

function isCurrentRunId(runId) {
  return !!runId && state.run?.id === runId;
}

function getOrCreatePlayerId() {
  let id = localStorage.getItem(PLAYER_ID_KEY);
  if (id) return id;
  id = crypto.randomUUID ? crypto.randomUUID() : `${Date.now()}-${Math.random().toString(16).slice(2)}`;
  localStorage.setItem(PLAYER_ID_KEY, id);
  return id;
}

const AUTH_ERROR_TEXT = {
  username_taken: "that username is taken",
  username_invalid: "username must be 3–24 chars (letters, numbers, _ or -)",
  password_too_short: "password must be at least 6 characters",
  invalid_credentials: "wrong username or password",
  rate_limited: "too many attempts — try again in a minute",
  already_registered: "this account is already registered",
  player_id_required: "missing player id",
  server_error: "server error — please try again",
};

function authErrText(code) {
  return AUTH_ERROR_TEXT[code] || code || "something went wrong";
}

async function fetchWhoami() {
  try {
    const url = `/api/whoami?player_id=${encodeURIComponent(state.playerId || "")}`;
    const res = await fetch(url, { credentials: "same-origin" });
    if (!res.ok) return null;
    return await res.json();
  } catch {
    return null;
  }
}

function applyWhoami(w) {
  if (!w) {
    state.auth = { username: null, displayName: null, avatar: null, hasAccount: false, signedIn: false, edgeCase: false };
    return;
  }
  if (w.player_id && w.player_id !== state.playerId) {
    state.playerId = w.player_id;
    localStorage.setItem(PLAYER_ID_KEY, w.player_id);
    state.profile.player_id = w.player_id;
  }
  state.auth = {
    username: w.username || null,
    displayName: w.display_name || null,
    avatar: w.avatar || null,
    hasAccount: !!w.has_account,
    signedIn: !!w.signed_in,
    edgeCase: !!w.has_account && !w.signed_in,
  };
}

async function postJson(url, body) {
  const res = await fetch(url, {
    method: "POST",
    credentials: "same-origin",
    headers: { "Content-Type": "application/json" },
    body: JSON.stringify(body || {}),
  });
  let data = null;
  try { data = await res.json(); } catch { /* may be empty */ }
  return { ok: res.ok, status: res.status, data };
}

function clearReplayQueryParam() {
  const url = new URL(window.location.href);
  url.searchParams.delete("replay");
  history.replaceState(null, "", `${url.pathname}${url.search}${url.hash}`);
}

function replayShareUrl(replayId) {
  const url = new URL(window.location.href);
  url.searchParams.set("replay", String(replayId));
  return url.toString();
}

async function fetchSharedReplay(replayId) {
  try {
    const res = await fetch(`/api/replay?battle_id=${encodeURIComponent(replayId)}`, {
      credentials: "same-origin",
    });
    let data = null;
    try { data = await res.json(); } catch { /* noop */ }
    return { ok: res.ok, status: res.status, data };
  } catch {
    return { ok: false, status: 0, data: null };
  }
}

async function registerAccount({ username, password }) {
  return postJson("/api/register", { player_id: state.playerId, username, password });
}

async function loginAccount({ username, password, stay }) {
  return postJson("/api/login", { username, password, stay: !!stay });
}

async function logoutAccount() {
  return postJson("/api/logout", {});
}

function reconnectWs() {
  // Drop the current socket so the next upgrade reads the latest cookie state.
  if (state.ws) {
    try { state.ws.close(); } catch { /* noop */ }
    state.ws = null;
  }
  connect();
}

function showLoginScreen() {
  $("#loginError").textContent = "";
  $("#loginUsername").value = "";
  $("#loginPassword").value = "";
  $("#loginStay").checked = false;
  show("login");
  setTimeout(() => $("#loginUsername")?.focus(), 0);
}

function renderStartScreen() {
  const normal = $("#startNormal");
  const edge = $("#startEdgeCase");
  if (!normal || !edge) return;
  if (state.auth.edgeCase) {
    normal.classList.add("hidden");
    edge.classList.remove("hidden");
    const avatarId = state.auth.avatar || DEFAULT_AVATAR_ID;
    const avatar = profileAvatarDef(avatarId);
    const img = $("#edgeCaseAvatar");
    if (img) {
      img.src = assetHref(avatar.sprite);
      img.alt = avatar.name;
    }
    $("#edgeCaseName").textContent = state.auth.displayName || "anon";
    $("#edgeCaseUsername").textContent = state.auth.username ? `@${state.auth.username}` : "";
    $("#edgeCaseError").textContent = "";
    $("#edgeCasePassword").value = "";
    $("#edgeCaseStay").checked = false;
  } else {
    normal.classList.remove("hidden");
    edge.classList.add("hidden");
  }
}

function renderProfileAccountControls() {
  const status = $("#profileAccountStatus");
  const registerBtn = $("#registerBtn");
  const logoutBtn = $("#logoutBtn");
  const panel = $("#registerPanel");
  if (!status || !registerBtn || !logoutBtn || !panel) return;
  if (state.auth.signedIn && state.auth.username) {
    status.textContent = `@${state.auth.username}`;
    registerBtn.classList.add("hidden");
    logoutBtn.classList.remove("hidden");
    panel.classList.add("hidden");
  } else if (state.auth.hasAccount) {
    status.textContent = `@${state.auth.username || ""} — signed out`;
    registerBtn.classList.add("hidden");
    logoutBtn.classList.add("hidden");
    panel.classList.add("hidden");
  } else {
    status.textContent = "guest — register to save progress across devices.";
    registerBtn.classList.remove("hidden");
    logoutBtn.classList.add("hidden");
  }
}

function openRegisterPanel() {
  $("#registerError").textContent = "";
  $("#registerUsername").value = "";
  $("#registerPassword").value = "";
  $("#registerPasswordConfirm").value = "";
  $("#registerPanel").classList.remove("hidden");
  $("#registerBtn").classList.add("hidden");
  setTimeout(() => $("#registerUsername")?.focus(), 0);
}

function closeRegisterPanel() {
  $("#registerPanel").classList.add("hidden");
  if (!state.auth.hasAccount) $("#registerBtn").classList.remove("hidden");
}

async function handleRegisterSubmit(e) {
  e.preventDefault();
  const username = $("#registerUsername").value.trim();
  const password = $("#registerPassword").value;
  const confirm = $("#registerPasswordConfirm").value;
  const errEl = $("#registerError");
  errEl.textContent = "";
  if (password !== confirm) {
    errEl.textContent = "passwords don't match";
    return;
  }
  if (password.length < 6) {
    errEl.textContent = authErrText("password_too_short");
    return;
  }
  const { ok, data } = await registerAccount({ username, password });
  if (!ok) {
    errEl.textContent = authErrText(data?.error);
    return;
  }
  // Successful: cookie is set by server. Refresh whoami, close panel, reconnect WS.
  const w = await fetchWhoami();
  applyWhoami(w);
  renderProfileAccountControls();
  renderStartScreen();
  closeRegisterPanel();
  closeProfileModal();
  flash(`account @${state.auth.username} registered — write your password down!`, { variant: "info", duration: 5000 });
  reconnectWs();
}

async function handleLoginSubmit({ username, password, stay, errEl, onSuccess }) {
  errEl.textContent = "";
  if (!username || !password) {
    errEl.textContent = authErrText("invalid_credentials");
    return;
  }
  const { ok, data } = await loginAccount({ username, password, stay });
  if (!ok) {
    errEl.textContent = authErrText(data?.error);
    return;
  }
  if (data?.player_id) {
    state.playerId = data.player_id;
    localStorage.setItem(PLAYER_ID_KEY, data.player_id);
    state.profile.player_id = data.player_id;
  }
  const w = await fetchWhoami();
  applyWhoami(w);
  renderProfileAccountControls();
  renderStartScreen();
  if (typeof onSuccess === "function") onSuccess();
  reconnectWs();
}

async function handleLogout() {
  await logoutAccount();
  // Logging out: keep current playerId in localStorage so we land in the edge-case
  // flow next boot. (User stays "associated with" their account on this device.)
  await refreshAuthAndRender();
  reconnectWs();
}

async function refreshAuthAndRender() {
  const w = await fetchWhoami();
  applyWhoami(w);
  renderProfileAccountControls();
  renderStartScreen();
}

function signInAsAnotherUser() {
  // The current localStorage UUID belongs to a registered account that isn't authed here.
  // Wipe it so we boot fresh as a brand-new guest UUID.
  localStorage.removeItem(PLAYER_ID_KEY);
  localStorage.removeItem(PLAYER_ID_KEY + "_old");
  location.reload();
}

function getStoredBgmVolume() {
  const n = Number(localStorage.getItem(BGM_VOLUME_KEY));
  if (!Number.isFinite(n)) return 0.35;
  return Math.max(0, Math.min(1, n));
}

function ensureBgmAudio() {
  if (state.bgmAudio) return state.bgmAudio;
  const audio = new Audio(assetHref("staticeulogy.opus"));
  audio.loop = true;
  audio.preload = "auto";
  state.bgmAudio = audio;
  syncBgmControls();
  return audio;
}

function ensureBgmAltAudio() {
  if (state.bgmAltAudio) return state.bgmAltAudio;
  const audio = new Audio(assetHref("staticeulogy-deepfried.opus"));
  audio.loop = true;
  audio.preload = "auto";
  state.bgmAltAudio = audio;
  syncBgmControls();
  return audio;
}

function usableBgmVolume() {
  return state.bgmMuted || state.bgmVolume <= 0 ? 0 : state.bgmVolume;
}

function applyBgmMix() {
  const volume = usableBgmVolume();
  const muted = volume <= 0;
  const mix = Math.max(0, Math.min(1, state.bgmAltMix || 0));
  if (state.bgmAudio) {
    state.bgmAudio.volume = volume * (1 - mix);
    state.bgmAudio.muted = muted;
  }
  if (state.bgmAltAudio) {
    state.bgmAltAudio.volume = volume * mix;
    state.bgmAltAudio.muted = muted;
  }
}

function alignBgmTime(source, target) {
  if (!source || !target) return;
  const apply = () => {
    const sourceTime = Number.isFinite(source.currentTime) ? source.currentTime : 0;
    const targetDuration = Number.isFinite(target.duration) && target.duration > 0 ? target.duration : null;
    const targetTime = targetDuration ? sourceTime % targetDuration : sourceTime;
    try {
      target.currentTime = Math.max(0, targetTime);
    } catch {
      // Some browsers reject currentTime before metadata is ready; the fade still works.
    }
  };
  if (target.readyState >= 1) apply();
  else target.addEventListener("loadedmetadata", apply, { once: true });
}

function finishBgmFade() {
  state.bgmFadeRaf = null;
  applyBgmMix();
  if (usableBgmVolume() <= 0) {
    state.bgmAudio?.pause();
    state.bgmAltAudio?.pause();
  } else if (state.bgmAltMix >= 1) {
    state.bgmAudio?.pause();
  } else if (state.bgmAltMix <= 0) {
    state.bgmAltAudio?.pause();
  }
}

function playBgmForMix(playBoth = false) {
  if (usableBgmVolume() <= 0) return;
  if (playBoth || state.bgmAltMix < 1) state.bgmAudio?.play().catch(() => {});
  if (playBoth || state.bgmAltActive || state.bgmAltMix > 0) state.bgmAltAudio?.play().catch(() => {});
}

function setBgmAltActive(active) {
  const targetMix = active ? 1 : 0;
  if (state.bgmAltActive === active && state.bgmAltMix === targetMix) return;

  state.bgmAltActive = active;
  if (state.bgmFadeRaf) {
    cancelAnimationFrame(state.bgmFadeRaf);
    state.bgmFadeRaf = null;
  }

  const normal = ensureBgmAudio();
  const alt = ensureBgmAltAudio();
  if (active) alignBgmTime(normal, alt);
  else alignBgmTime(alt, normal);

  const startMix = Math.max(0, Math.min(1, state.bgmAltMix || 0));
  const startedAt = performance.now();
  applyBgmMix();
  playBgmForMix(true);

  const step = (now) => {
    const t = Math.min(1, (now - startedAt) / BGM_CROSSFADE_MS);
    state.bgmAltMix = startMix + (targetMix - startMix) * t;
    applyBgmMix();
    if (t < 1) {
      state.bgmFadeRaf = requestAnimationFrame(step);
    } else {
      state.bgmAltMix = targetMix;
      finishBgmFade();
    }
  };
  state.bgmFadeRaf = requestAnimationFrame(step);
}

function syncBgmControls() {
  applyBgmMix();
  const slider = $("#bgmVolume");
  if (slider) {
    const volumePct = Math.round(state.bgmVolume * 100);
    slider.value = String(volumePct);
    slider.style.setProperty("--bgm-volume-pct", `${volumePct}%`);
  }
  const btn = $("#bgmMuteBtn");
  if (!btn) return;
  const muted = state.bgmMuted || state.bgmVolume <= 0;
  btn.textContent = muted ? "🔇" : "🔊";
  btn.setAttribute("aria-pressed", muted ? "true" : "false");
  btn.setAttribute("aria-label", muted ? "unmute background music" : "mute background music");
}

function startBgm() {
  if (state.bgmMuted || state.bgmVolume <= 0) return;
  ensureBgmAudio();
  if (state.bgmAltActive || state.bgmAltMix > 0) ensureBgmAltAudio();
  syncBgmControls();
  playBgmForMix();
}

function setBgmMuted(muted) {
  state.bgmMuted = muted;
  if (!muted && state.bgmVolume <= 0) {
    state.bgmVolume = 0.35;
    localStorage.setItem(BGM_VOLUME_KEY, String(state.bgmVolume));
  }
  localStorage.setItem(BGM_MUTED_KEY, muted ? "1" : "0");
  syncBgmControls();
  if (muted) {
    state.bgmAudio?.pause();
    state.bgmAltAudio?.pause();
  } else {
    startBgm();
  }
}

function setBgmVolume(value) {
  state.bgmVolume = Math.max(0, Math.min(1, Number(value) / 100 || 0));
  localStorage.setItem(BGM_VOLUME_KEY, String(state.bgmVolume));
  if (state.bgmVolume > 0 && state.bgmMuted) {
    state.bgmMuted = false;
    localStorage.setItem(BGM_MUTED_KEY, "0");
  }
  syncBgmControls();
  startBgm();
}

function setConnectionStatus(label, variant) {
  const el = $("#connectionPill");
  if (!el) return;
  el.textContent = label;
  el.classList.remove("connection-pill--connecting", "connection-pill--online", "connection-pill--offline");
  el.classList.add(`connection-pill--${variant}`);
}

function syncSiteStats(s) {
  if (!s || typeof s.active_players !== "number" || typeof s.logged_in_today !== "number") return;
  $("#statActivePlayers").textContent = String(s.active_players);
  $("#statLoggedToday").textContent = String(s.logged_in_today);
}

function setNickname(name, notifyServer = true) {
  const next = (name || "anon").trim().slice(0, 24) || "anon";
  state.nickname = next;
  state.profile.name = next;
  localStorage.setItem(NICKNAME_KEY, next);
  const profileInput = $("#profileNameInput");
  if (profileInput && document.activeElement !== profileInput) profileInput.value = next;
  if (state.run) state.run.name = next;
  if (notifyServer && state.run) send({ type: "rename_player", name: next });
  renderProfilePill();
}

function profileAvatarDef(id = state.profile?.selected_avatar) {
  return state.profileAvatars.find((a) => a.id === id)
    || state.profileAvatars.find((a) => a.id === DEFAULT_AVATAR_ID)
    || { id: DEFAULT_AVATAR_ID, name: "Meme Man", sprite: "Meme_Man.webp", required_wins: 0, required_ultimate_victories: 0 };
}

function avatarUnlocked(avatar) {
  if (!avatar) return false;
  return (state.profile.best_wins || 0) >= (avatar.required_wins || 0)
    && (state.profile.ultimate_victories || 0) >= (avatar.required_ultimate_victories || 0);
}

function avatarRequirementText(avatar) {
  if (!avatar || avatarUnlocked(avatar)) return "unlocked";
  if (avatar.required_ultimate_victories) {
    return `${avatar.required_ultimate_victories} ultimate victor${avatar.required_ultimate_victories === 1 ? "y" : "ies"}`;
  }
  return `reach ${avatar.required_wins} wins`;
}

function avatarImgHtml(avatarId, className = "identity-avatar") {
  const avatar = profileAvatarDef(avatarId);
  return `<span class="${className}"><img src="${assetHref(avatar.sprite)}" alt="${escape(avatar.name)}" /></span>`;
}

function characterStatsHtml(cd) {
  return `<div class="stats char-stats">
    <span title="might">⚔${cd.might}</span>
    <span title="quickness">⚡${cd.reflexes}</span>
    <span title="wisdom">✦${cd.wisdom}</span>
    <span title="hp">❤${cd.hp}</span>
  </div>`;
}

function applyProfile(profile) {
  if (!profile) return;
  state.profile = { ...state.profile, ...profile };
  state.profile.player_id = state.profile.player_id || state.playerId;
  state.nickname = state.profile.name || "anon";
  localStorage.setItem(NICKNAME_KEY, state.nickname);
  renderProfilePill();
  renderAvatarGrid();
  const profileInput = $("#profileNameInput");
  if (profileInput && document.activeElement !== profileInput) profileInput.value = state.nickname;
  if (state.run) state.run.name = state.nickname;
}

function renderProfilePill() {
  const avatar = profileAvatarDef(state.profile?.selected_avatar);
  const img = $("#profilePillAvatar");
  if (img) {
    img.src = assetHref(avatar.sprite);
    img.alt = avatar.name;
  }
  const name = $("#profilePillName");
  if (name) name.textContent = state.profile?.name || state.nickname || "anon";
  const modalImg = $("#profileModalAvatar");
  if (modalImg) {
    modalImg.src = assetHref(profileAvatarDef(state.profileDraftAvatar || state.profile?.selected_avatar).sprite);
    modalImg.alt = profileAvatarDef(state.profileDraftAvatar || state.profile?.selected_avatar).name;
  }
  renderProfileStats();
}

function renderProfileStats() {
  const stats = $("#profileStats");
  if (!stats) return;
  const mmr = state.run?.mmr ?? state.lb.playerMmr;
  const rank = state.lb.playerRank;
  const items = [
    { label: "mmr", value: mmr != null ? mmr : "—" },
    { label: "rank", value: rank != null ? `#${rank}` : "—" },
    { label: "best", value: `${state.profile.best_wins || 0}w` },
    { label: "ult ×", value: state.profile.ultimate_victories || 0 },
  ];
  stats.innerHTML = items.map((s) =>
    `<li><span class="profile-stat-label">${s.label}</span><span class="profile-stat-value">${s.value}</span></li>`
  ).join("");
}

function renderAvatarGrid() {
  const grid = $("#profileAvatarGrid");
  if (!grid || !state.profileAvatars.length) return;
  const selected = state.profileDraftAvatar || state.profile.selected_avatar || DEFAULT_AVATAR_ID;
  grid.innerHTML = state.profileAvatars.map((avatar) => {
    const unlocked = avatarUnlocked(avatar);
    const isSelected = avatar.id === selected;
    return `<button class="profile-avatar-card${unlocked ? "" : " locked"}${isSelected ? " selected" : ""}" type="button" data-avatar-id="${escape(avatar.id)}" ${unlocked ? "" : "disabled"}>
      <span class="profile-avatar profile-avatar--grid"><img src="${assetHref(avatar.sprite)}" alt="${escape(avatar.name)}" /></span>
      <span class="profile-avatar-requirement">${escape(avatarRequirementText(avatar))}</span>
    </button>`;
  }).join("");
  grid.querySelectorAll("[data-avatar-id]").forEach((btn) => {
    btn.addEventListener("click", () => {
      const selectedAvatar = btn.dataset.avatarId;
      state.profileDraftAvatar = selectedAvatar;
      state.profile.selected_avatar = selectedAvatar;
      renderProfilePill();
      renderAvatarGrid();
      saveProfile();
    });
  });
}

function openProfileModal() {
  state.profileDraftAvatar = state.profile.selected_avatar || DEFAULT_AVATAR_ID;
  $("#profileNameInput").value = state.profile.name || state.nickname || "anon";
  renderProfilePill();
  renderAvatarGrid();
  renderProfileAccountControls();
  closeRegisterPanel();
  $("#profileModal").classList.remove("hidden");
  $("#profileNameInput")?.focus();
  // Refresh rank/mmr in the background; the leaderboard handler updates state
  // even when the lb modal is closed.
  send({
    type: "leaderboard",
    page: 1,
    per_page: LB_PAGE_SIZE,
    around_player_id: state.playerId,
  });
}

function closeProfileModal() {
  $("#profileModal").classList.add("hidden");
  $("#profilePill")?.focus();
}

function openHelpModal() {
  $("#helpModal").classList.remove("hidden");
  $("#helpCloseBtn")?.focus();
}

function closeHelpModal() {
  $("#helpModal").classList.add("hidden");
  $("#helpBtn")?.focus();
}

function saveProfile() {
  const name = ($("#profileNameInput").value || state.nickname || "anon").trim().slice(0, 24) || "anon";
  state.profileDraftAvatar = state.profileDraftAvatar || state.profile.selected_avatar || DEFAULT_AVATAR_ID;
  setNickname(name, false);
  send({
    type: "set_profile",
    player_id: state.playerId,
    name,
    selected_avatar: state.profileDraftAvatar,
  });
}

function stopGameOverReplay() {
  $("#goReplayCanvas").__battleTooltipCleanup?.();
  state.gameOverReplayKey = null;
}

function show(screenId) {
  if (screenId !== "gameover") {
    stopGameOverReplay();
    setBgmAltActive(false);
  }
  $$(".screen").forEach((s) => s.classList.add("hidden"));
  $(`#${screenId}`).classList.remove("hidden");
  syncRunHudVisibility();
}

/** HUD is outside individual screens so wins/losses stay visible during battle playback. */
function syncRunHudVisibility() {
  const hud = $("#runHud");
  if (!hud) return;
  const r = state.run;
  const shopOn = $("#shop") && !$("#shop").classList.contains("hidden");
  const battleOn = $("#battle") && !$("#battle").classList.contains("hidden");
  hud.classList.toggle("hidden", !(r && (shopOn || battleOn)));
}

function syncRunHudValues() {
  const r = state.run;
  if (!r) return;
  $("#hudMoney").textContent = r.money;
  $("#hudWins").textContent = r.wins;
  $("#hudLosses").textContent = r.losses;
  $("#hudMmr").textContent = r.mmr ?? "????";
}

function isSharedReplayActive() {
  return !!state.sharedReplay.activeId;
}

function currentReplayId() {
  const replayId = state.lastBattle?.replay_id;
  return Number.isInteger(replayId) && replayId > 0 ? replayId : null;
}

function syncReplayLinkButtons() {
  const replayId = currentReplayId();
  $("#copyReplayLinkBtn")?.classList.toggle("hidden", !(replayId && !state.battleAnimating));
  const hasGameOverReplay = !!(replayId && state.lastBattle?.events?.length);
  $("#copyGoReplayLinkBtn")?.classList.toggle("hidden", !hasGameOverReplay);
}

function logSharedReplayResult(battleMsg) {
  const log = (text) => {
    const d = document.createElement("div");
    d.textContent = text;
    $("#battleLog").appendChild(d);
    $("#battleLog").scrollTop = 1e9;
  };
  if (battleMsg.winner === 0) {
    log(`✦ ${battleMsg.player_name || "left side"} wins`);
  } else if (battleMsg.winner === 1) {
    log(`✦ ${battleMsg.opponent_name || "right side"} wins`);
  } else {
    log("— draw —");
  }
}

function finishSharedReplayPlayback(battleMsg) {
  state.battleAnimating = false;
  logSharedReplayResult(battleMsg);
  $("#nextRoundBtn").textContent = "enter arcade";
  $("#nextRoundBtn").classList.remove("hidden");
  $("#replayBattleBtn").classList.remove("hidden");
  syncReplayLinkButtons();
}

function openSharedReplay(battleMsg) {
  state.sharedReplay.activeId = battleMsg.replay_id;
  state.lastBattle = battleMsg;
  state.run = null;
  state.battleAnimating = true;
  stopGameOverReplay();
  $("#battleCanvas").__battleTooltipCleanup?.();
  $("#battleLog").innerHTML = "";
  renderIdentity($("#leftName"), battleMsg.player_name || "left side", battleMsg.player_mmr_before, battleMsg.player_avatar || DEFAULT_AVATAR_ID);
  renderIdentity($("#rightName"), battleMsg.opponent_name || "right side", battleMsg.opponent_mmr_before, battleMsg.opponent_avatar || DEFAULT_AVATAR_ID);
  $("#nextRoundBtn").classList.add("hidden");
  $("#replayBattleBtn").classList.add("hidden");
  syncReplayLinkButtons();
  show("battle");
  playBattle($("#battleCanvas"), battleMsg, charDef, itemDef, () => {
    finishSharedReplayPlayback(battleMsg);
  }, {
    showTooltip: (reference, sprite) => showTooltip(reference, combatantTooltip(sprite)),
    hideTooltipNow,
  });
  if (battleMsg.version_mismatch) {
    flash("replay may differ slightly — combat rules changed since it was recorded", {
      variant: "info",
      duration: 4200,
    });
  }
}

function leaveSharedReplayMode() {
  state.sharedReplay.requestedId = null;
  state.sharedReplay.activeId = null;
  state.sharedReplay.loading = false;
  state.lastBattle = null;
  state.battleAnimating = false;
  clearReplayQueryParam();
  $("#battleCanvas").__battleTooltipCleanup?.();
  $("#battleLog").innerHTML = "";
  $("#nextRoundBtn").textContent = "continue";
  $("#nextRoundBtn").classList.add("hidden");
  $("#replayBattleBtn").classList.add("hidden");
  syncReplayLinkButtons();
}

async function copyText(text) {
  if (navigator.clipboard?.writeText) {
    await navigator.clipboard.writeText(text);
    return;
  }
  const input = document.createElement("textarea");
  input.value = text;
  input.setAttribute("readonly", "");
  input.style.position = "fixed";
  input.style.opacity = "0";
  document.body.appendChild(input);
  input.select();
  document.execCommand("copy");
  input.remove();
}

async function copyCurrentReplayLink() {
  const replayId = currentReplayId();
  if (!replayId) return;
  try {
    await copyText(replayShareUrl(replayId));
    flash("link copied", { variant: "info", duration: 1800 });
  } catch {
    flash("couldn't copy link", { variant: "error", duration: 2600 });
  }
}

async function loadRequestedReplay() {
  const replayId = state.sharedReplay.requestedId;
  if (!replayId || state.sharedReplay.loading || state.sharedReplay.activeId) return false;
  state.sharedReplay.loading = true;
  const res = await fetchSharedReplay(replayId);
  state.sharedReplay.loading = false;
  if (!res.ok || !res.data) {
    flash("replay not found", { variant: "error", duration: 2800 });
    leaveSharedReplayMode();
    if (state.auth.edgeCase) {
      renderStartScreen();
      show("start");
    } else {
      resumeRun(true);
    }
    return false;
  }
  openSharedReplay(res.data);
  return true;
}

function showSessionReplacedBanner() {
  if (document.getElementById("sessionReplacedBanner")) return;
  const overlay = document.createElement("div");
  overlay.id = "sessionReplacedBanner";
  overlay.style.cssText =
    "position:fixed;inset:0;background:rgba(8,6,18,0.86);z-index:9999;" +
    "display:flex;align-items:center;justify-content:center;font-family:inherit;color:#f4f1ff;";
  const card = document.createElement("div");
  card.style.cssText =
    "max-width:24rem;padding:1.5rem 1.75rem;border-radius:0.75rem;" +
    "background:#1d1638;border:1px solid #4a3a8a;text-align:center;line-height:1.45;";
  const title = document.createElement("div");
  title.style.cssText = "font-size:1.1rem;margin-bottom:0.5rem;font-weight:600;";
  title.textContent = "this run is being played in another tab";
  const body = document.createElement("div");
  body.style.cssText = "font-size:0.9rem;opacity:0.85;margin-bottom:1.25rem;";
  body.textContent =
    "to avoid scrambling the save, only one tab can play at a time. " +
    "reload here to take the run back.";
  const btn = document.createElement("button");
  btn.textContent = "reload";
  btn.style.cssText =
    "padding:0.55rem 1.25rem;font-size:0.95rem;border-radius:0.4rem;" +
    "background:#6b54d6;color:#fff;border:none;cursor:pointer;";
  btn.addEventListener("click", () => location.reload());
  card.append(title, body, btn);
  overlay.append(card);
  document.body.append(overlay);
  setTimeout(() => btn.focus(), 0);
}

function connect() {
  const proto = location.protocol === "https:" ? "wss" : "ws";
  const ws = new WebSocket(`${proto}://${location.host}${BASE_PATH}/ws`);
  state.ws = ws;
  setConnectionStatus("connecting", "connecting");
  ws.onopen = () => {
    if (state.ws === ws) setConnectionStatus("online", "online");
  };
  ws.onmessage = (ev) => {
    const msg = JSON.parse(ev.data);
    handleServer(msg);
  };
  ws.onclose = () => {
    if (state.ws !== ws) return;
    if (state.evicted) {
      setConnectionStatus("offline", "offline");
      return;
    }
    setConnectionStatus("reconnecting", "offline");
    setTimeout(connect, 1000);
  };
  ws.onerror = () => {
    if (state.ws === ws) setConnectionStatus("offline", "offline");
  };
}
function send(obj) {
  if (state.ws?.readyState !== WebSocket.OPEN) return false;
  state.ws.send(JSON.stringify(obj));
  return true;
}

function resumeRun(quiet = false) {
  state.autoResumePending = quiet;
  return send({ type: "resume", player_id: state.playerId });
}

function startNewRun() {
  if (state.sharedReplay.requestedId != null || state.sharedReplay.activeId != null) {
    leaveSharedReplayMode();
  }
  setNickname(state.profile.name || state.nickname, false);
  state.lastBattle = null;
  state.battleAnimating = false;
  stopGameOverReplay();
  $("#battleCanvas").__battleTooltipCleanup?.();
  return send({ type: "new_run", player_id: state.playerId, name: state.nickname });
}

function openQuitRunModal() {
  if (!state.run) return;
  const modal = $("#quitRunModal");
  modal.classList.remove("hidden");
  $("#confirmQuitRunBtn").focus();
}

function closeQuitRunModal() {
  $("#quitRunModal").classList.add("hidden");
  $("#quitRunBtn")?.focus();
}

function handleServer(msg) {
  switch (msg.type) {
    case "defs":
      state.defs.characters = msg.characters;
      state.defs.items = msg.items;
      state.profileAvatars = msg.profile_avatars || [];
      state.consts = msg.constants;
      $("#hudMaxLosses").textContent = msg.constants.max_losses;
      $("#goMaxWins").textContent = msg.constants.max_wins;
      $("#rerollCost").textContent = msg.constants.reroll_cost;
      if (msg.site_stats) syncSiteStats(msg.site_stats);
      renderProfilePill();
      // Edge case present: don't auto-resume — server would just reject with auth_required.
      // The user has to log in first.
      if (state.sharedReplay.requestedId != null) {
        loadRequestedReplay();
      } else if (state.auth.edgeCase) {
        renderStartScreen();
      } else {
        resumeRun(true);
      }
      break;
    case "auth_required":
      state.autoResumePending = false;
      state.run = null;
      state.lastBattle = null;
      refreshAuthAndRender().then(() => {
        show("start");
        if (state.auth.edgeCase) {
          flash("please sign in to continue", { variant: "info", duration: 3000 });
        }
      });
      break;
    case "session_replaced":
      state.evicted = true;
      state.autoResumePending = false;
      try { state.ws?.close(); } catch (_) {}
      showSessionReplacedBanner();
      break;
    case "profile":
      applyProfile(msg.profile);
      break;
    case "state": {
      const wasAutoResume = state.autoResumePending;
      state.autoResumePending = false;
      state.run = msg.run;
      applyProfile(msg.profile || { name: msg.run.name, selected_avatar: state.profile.selected_avatar });
      // A quiet auto-resume that lands on a stale game_over (e.g. a registered
      // user logging back in days later) should jump straight into a new run
      // instead of showing the old GG screen.
      if (wasAutoResume && msg.run.phase === "game_over") {
        startNewRun();
        break;
      }
      renderRun();
      break;
    }
    case "battle":
      if (state.run && !isCurrentRunId(msg.run_id)) break;
      state.lastBattle = msg;
      state.sharedReplay.activeId = null;
      const battleRunId = msg.run_id;
      // Keep pre-battle HUD values during replay; the post-battle snapshot is
      // applied only once the animation finishes so the result isn't spoiled.
      const preBattleRun = state.run ? { ...state.run } : null;
      const postBattleRun = msg.run;
      const applyBattleSnapshot = (m) => {
        if (!isCurrentRunId(battleRunId)) return false;
        if (m.run) {
          state.run = m.run;
        } else {
          Object.assign(state.run, {
            phase: m.phase,
            wins: m.wins,
            losses: m.losses,
            alive: m.alive,
            money: m.money_after,
          });
        }
        return true;
      };
      if (preBattleRun) {
        state.run = preBattleRun;
      } else {
        state.run = postBattleRun;
      }
      syncRunHudValues();
      show("battle");
      renderIdentity($("#leftName"), state.run?.name ?? "you", msg.player_mmr_before, msg.player_avatar || state.profile.selected_avatar);
      renderIdentity($("#rightName"), msg.opponent_name, msg.opponent_mmr_before, msg.opponent_avatar || DEFAULT_AVATAR_ID, {
        unknownMmr: msg.opponent_mmr_before == null,
      });
      $("#nextRoundBtn").classList.add("hidden");
      $("#replayBattleBtn").classList.add("hidden");
      $("#nextRoundBtn").textContent = "continue";
      $("#battleLog").innerHTML = "";
      state.battleAnimating = true;
      syncReplayLinkButtons();
      playBattle($("#battleCanvas"), msg, charDef, itemDef, () => {
        if (!isCurrentRunId(battleRunId)) return;
        state.battleAnimating = false;
        const log = (t) => {
          const d = document.createElement("div");
          d.textContent = t; $("#battleLog").appendChild(d); $("#battleLog").scrollTop = 1e9;
        };
        if (msg.winner === 0) log(`✦ you win — +$${state.consts.win_reward}`);
        else if (msg.winner === 1) log(`✗ defeat — +$${state.consts.lose_reward}`);
        else log(`— draw —`);
        if (state.lastBattle && state.run) {
          const m = state.lastBattle;
          applyBattleSnapshot(m);
          syncRunHudValues();
        }
        if (state.run?.phase !== "game_over") {
          $("#nextRoundBtn").classList.remove("hidden");
          $("#replayBattleBtn").classList.remove("hidden");
          syncReplayLinkButtons();
        } else {
          renderRun();
        }
      }, {
        showTooltip: (reference, sprite) => showTooltip(reference, combatantTooltip(sprite)),
        hideTooltipNow,
      });
      break;
    case "leaderboard":
      handleLeaderboardMsg(msg);
      break;
    case "site_stats":
      syncSiteStats(msg);
      break;
    case "error": {
      const m = msg.message;
      if (state.autoResumePending && m === "run not found") {
        state.autoResumePending = false;
        // Signed-in users skip the "enter the arcade" landing entirely — they've
        // already onboarded. Drop them into a new run automatically. First-time
        // guests stay on the start screen so they see the log-in option.
        if (state.auth.signedIn) startNewRun();
        break;
      }
      state.autoResumePending = false;
      const cashDenied =
        /^need \$[\d]+ more/i.test(m) ||
        /costs \$[\d]+, have \$/i.test(m);
      flash(m, {
        variant: "error",
        duration: cashDenied ? 3600 : 2600,
        shakeMoney: cashDenied,
      });
      break;
    }
  }
}

const escape = (s) => String(s).replace(/[&<>"]/g, c => ({"&":"&amp;","<":"&lt;",">":"&gt;",'"':"&quot;"}[c]));
function formatPlayerName(name, mmr, opts = {}) {
  const label = name || "anon";
  if (opts.unknownMmr) return `${label} (????)`;
  const value = Number(mmr);
  if (!Number.isFinite(value)) return label;
  return `${label} (${Math.round(value)})`;
}
function renderIdentity(el, name, mmr, avatarId, opts = {}) {
  if (!el) return;
  el.classList.add("identity-chip");
  el.innerHTML = `${avatarImgHtml(avatarId)}<span>${escape(formatPlayerName(name, mmr, opts))}</span>`;
}
const charDef = (id) => state.defs.characters.find(c => c.id === id);
const itemDef = (id) => state.defs.items.find(i => i.id === id);
/** `TeamMember` JSON field, websocket `ItemSlot` name, empty-socket label */
const HAND_ARM_ROWS = [
  { itemProp: "left_hand", itemSlot: "left_hand", label: "left" },
  { itemProp: "right_hand", itemSlot: "right_hand", label: "right" },
  { itemProp: "hand_3", itemSlot: "left2", label: "left2" },
  { itemProp: "hand_4", itemSlot: "right2", label: "right2" },
];
function activeHandArmRows(member) {
  const cd = member ? charDef(member.def_id) : null;
  const n = cd?.hand_slots ?? 2;
  return HAND_ARM_ROWS.slice(0, n);
}
const HAND_SLOT_SET = new Set(HAND_ARM_ROWS.map((r) => r.itemSlot));
function memberSockets(member) {
  return [{ itemProp: "hat", itemSlot: "hat", label: "hat" }, ...activeHandArmRows(member)];
}
const STAT_KEYS = [
  { key: "might", label: "might" },
  { key: "reflexes", label: "reflexes" },
  { key: "wisdom", label: "wisdom" },
  { key: "hp", label: "hp" },
];
let tooltipEl = null;
let cleanupTooltip = null;
let activeTooltipReference = null;
let activeTooltipHtml = "";
let tooltipHideTimer = null;
const tooltipHosts = new Set();

function ensureTooltip() {
  if (tooltipEl) return tooltipEl;
  tooltipEl = document.createElement("div");
  tooltipEl.id = "tooltip";
  tooltipEl.className = "tooltip hidden";
  tooltipEl.setAttribute("role", "tooltip");
  document.body.appendChild(tooltipEl);
  return tooltipEl;
}

function updateTooltipPosition(reference) {
  const el = ensureTooltip();
  computePosition(reference, el, {
    placement: "top",
    middleware: [offset(10), flip(), shift({ padding: 8 })],
  }).then(({ x, y }) => {
    Object.assign(el.style, { left: `${x}px`, top: `${y}px` });
  });
}

function showTooltip(reference, html) {
  if (!html) return;
  if (tooltipHideTimer) {
    clearTimeout(tooltipHideTimer);
    tooltipHideTimer = null;
  }
  const el = ensureTooltip();
  if (activeTooltipReference !== reference) {
    if (cleanupTooltip) cleanupTooltip();
    cleanupTooltip = autoUpdate(reference, el, () => updateTooltipPosition(reference));
    activeTooltipReference = reference;
  }
  if (activeTooltipHtml !== html) {
    el.innerHTML = html;
    activeTooltipHtml = html;
  }
  el.classList.remove("hidden");
  updateTooltipPosition(reference);
}

function hideTooltip() {
  if (tooltipHideTimer) clearTimeout(tooltipHideTimer);
  tooltipHideTimer = setTimeout(() => {
    tooltipHosts.forEach((host) => {
      if (!host.isConnected) tooltipHosts.delete(host);
    });
    const hovered = [...tooltipHosts]
      .filter((host) => host.matches(":hover"))
      .sort((a, b) => (a.contains(b) ? -1 : b.contains(a) ? 1 : 0));
    const activeHost = hovered[hovered.length - 1];

    if (activeHost) {
      activeHost._showTooltip?.();
      return;
    }

    hideTooltipNow();
  }, 30);
}

function hideTooltipNow() {
  if (tooltipHideTimer) {
    clearTimeout(tooltipHideTimer);
    tooltipHideTimer = null;
  }
  if (cleanupTooltip) cleanupTooltip();
  cleanupTooltip = null;
  activeTooltipReference = null;
  activeTooltipHtml = "";
  ensureTooltip().classList.add("hidden");
}

function attachTooltip(el, content) {
  const show = () => showTooltip(el, typeof content === "function" ? content() : content);
  el._showTooltip = show;
  tooltipHosts.add(el);
  el.addEventListener("pointerenter", show);
  el.addEventListener("focus", show);
  el.addEventListener("pointerleave", hideTooltip);
  el.addEventListener("blur", hideTooltip);
}

function signed(n) {
  return n > 0 ? `+${n}` : `${n}`;
}

function armourRatingFromProperties(properties = []) {
  let sum = 0;
  for (const p of properties) {
    if (p?.kind !== "armour") continue;
    sum += Number(p.value) || 0;
  }
  return sum;
}

/** Combined armour only (battle / merged shop view). */
function armourTotalSectionHtml(properties = []) {
  const rating = armourRatingFromProperties(properties);
  if (rating <= 0) return "";
  const r = escape(String(rating));
  return `
    <div class="tooltip-section">armour</div>
    <div class="tooltip-armour-line">armour ${r}, reduces damage taken by ${r}</div>`;
}

/** Character def + equipped item properties (shop roster tooltip). */
function mergedMemberCombatProperties(member) {
  const cd = charDef(member?.def_id);
  const props = [...(cd?.properties || [])];
  memberItems(member).forEach(({ item }) => {
    (item.properties || []).forEach((x) => props.push(x));
  });
  return props;
}

function statBonusFromProperties(properties = []) {
  const bonus = { might: 0, reflexes: 0, wisdom: 0, hp: 0 };
  properties.forEach((p) => {
    if (p.kind !== "stat_bonus") return;
    STAT_KEYS.forEach(({ key }) => { bonus[key] += p[key] || 0; });
  });
  return bonus;
}

function statParts(p) {
  const values = STAT_KEYS.map(({ key }) => p[key] || 0);
  if (values.every((v) => v === values[0]) && values[0] !== 0) {
    return [`all stats ${signed(values[0])}`];
  }
  return STAT_KEYS
    .map(({ key, label }) => (p[key] ? `${label} ${signed(p[key])}` : null))
    .filter(Boolean);
}

function memberItems(member) {
  return memberSockets(member)
    .map(({ itemProp }) => ({
      key: itemProp,
      item: member?.[itemProp] ? itemDef(member[itemProp]) : null,
    }))
    .filter(({ item }) => item);
}

function effectiveStats(member) {
  const cd = charDef(member.def_id);
  const base = Object.fromEntries(STAT_KEYS.map(({ key }) => [key, cd?.[key] || 0]));
  const bonus = { might: 0, reflexes: 0, wisdom: 0, hp: 0 };
  memberItems(member).forEach(({ item }) => {
    const itemBonus = statBonusFromProperties(item.properties);
    STAT_KEYS.forEach(({ key }) => { bonus[key] += itemBonus[key]; });
  });
  const total = Object.fromEntries(STAT_KEYS.map(({ key }) => [key, base[key] + bonus[key]]));
  return { base, bonus, total };
}

function propertyText(p) {
  switch (p.kind) {
    case "stat_bonus": {
      const parts = statParts(p);
      return parts.length ? parts.join(", ") : "no stat bonus";
    }
    case "ranged": return "ranged attack";
    case "healer": return "heals allies for wisdom (1 mana each, 20 mana per battle)";
    case "freeze_on_hit": return "freezes on hit";
    case "summon_on_enemy_death": return `summon ${escape(p.species)} when an enemy dies`;
    case "summon_on_ally_death": return `summon ${escape(p.species)} when an ally dies`;
    case "damage_enemy_on_death": return `on death: deal ${escape(String(p.might_multiplier))}x might to enemy front`;
    case "team_stats_on_death": return `on death: team gets all stats ${signed(p.amount)}`;
    case "might_on_ally_death": return (
      `when an ally dies: might ${signed(p.might)} for this battle`
    );
    case "stats_on_ally_death": {
      const parts = statParts(p);
      const s = parts.length ? parts.join(", ") : "stats";
      return `when an ally dies: ${s} for this battle`;
    }
    case "stats_on_kill": {
      const parts = statParts(p);
      const s = parts.length ? parts.join(", ") : "stats";
      return `when this unit gets a kill: ${s} for this battle`;
    }
    case "crit_strike": return `${escape(String(p.chance_percent))}% critical strike (double damage)`;
    case "revive_once": return "revive once at full HP";
    case "revive_at_back_once": return "revive once at full HP at the back of formation";
    case "stats_per_living_ally": {
      const n = Number(p.amount) || 0;
      return `+${escape(String(n))} to all stats per other living ally`;
    }
    case "melee_cleave": {
      const n = Number(p.count);
      return `melee hits front ${escape(String(n))} enemies`;
    }
    case "melee_cleave_bonus":
      return `+${escape(String(p.plus))} melee cleave target${Number(p.plus) === 1 ? "" : "s"}`;
    case "melee_from_second": return "melee from second formation slot (overrides ranged)";
    case "buff_formation_front": {
      const parts = statParts(p);
      const bonus = parts.length ? parts.join(", ") : "stats";
      return (
        `front ally gets ${bonus} while this unit lives`
      );
    }
    case "armour": {
      const v = Number(p.value) || 0;
      const s = escape(String(v));
      return `armour ${s}, reduces damage taken by ${s}`;
    }
    case "debuff_enemy_reflexes": {
      const n = escape(String(p.amount ?? 0));
      return `enemies get −${n} quickness while this is held`;
    }
    case "drain_enemy_stats_on_hit": {
      const n = Number(p.amount) || 0;
      return `+${escape(String(n))} damage to all stats`;
    }
    default: return escape(p.kind || "property");
  }
}

function propertyLiHtml(p) {
  if (p.kind === "armour") {
    const v = Number(p.value) || 0;
    const s = escape(String(v));
    return `<li class="tooltip-prop-li">armour ${s}, reduces damage taken by ${s}</li>`;
  }
  return `<li>${propertyText(p)}</li>`;
}

function propertyList(properties = []) {
  if (!properties.length) return `<div class="tooltip-empty">no properties</div>`;
  return `<ul class="tooltip-props">${properties.map((p) => propertyLiHtml(p)).join("")}</ul>`;
}

function statGrid(base, total = base, currentHp = null) {
  return `<div class="tooltip-stats">
    ${STAT_KEYS.map(({ key, label }) => {
      const delta = total[key] - base[key];
      const value = key === "hp" && currentHp !== null ? `${currentHp}/${total[key]}` : total[key];
      let breakdown = "";
      if (delta !== 0) {
        const cls = delta > 0 ? "tooltip-delta--pos" : "tooltip-delta--neg";
        const abs = Math.abs(delta);
        const op = delta > 0 ? " + " : " − ";
        breakdown = ` <span class="tooltip-stat-breakdown">(<span class="tooltip-base">${base[key]}</span><span class="tooltip-stat-op">${op}</span><span class="tooltip-delta ${cls}">${abs}</span>)</span>`;
      }
      return `<div><span class="tooltip-stat-label">${label}</span><span class="tooltip-stat-value"><b>${value}</b>${breakdown}</span></div>`;
    }).join("")}
  </div>`;
}

function itemIcon(item, label = "") {
  return `<span class="tooltip-item-icon"><img src="${assetHref(item.sprite)}" alt="${escape(item.name)}" />${label ? `<span>${escape(label)}</span>` : ""}</span>`;
}

function itemTooltip(item) {
  if (!item) return "";
  return `<div class="tooltip-title">${escape(item.name)}</div>
    <div class="tooltip-meta">$${item.cost} · ${escape(item.slot)}</div>
    <div class="tooltip-hero">${itemIcon(item)}</div>
    <div class="tooltip-section">properties</div>
    ${propertyList(item.properties)}`;
}

function characterTooltip(cd) {
  if (!cd) return "";
  return `<div class="tooltip-title">${escape(cd.name)}</div>
    <div class="tooltip-meta">$${cd.cost}</div>
    <div class="tooltip-hero"><img src="${assetHref(cd.sprite)}" alt="${escape(cd.name)}" /></div>
    ${statGrid(cd)}
    <div class="tooltip-section">properties</div>
    ${propertyList(cd.properties)}`;
}

function memberTooltip(member) {
  const cd = charDef(member.def_id);
  if (!cd) return "";
  const stats = effectiveStats(member);
  const items = memberItems(member);
  const itemRows = items.length
    ? items.map(({ item }) => `<div class="tooltip-equipped">${itemIcon(item)}<div><b>${escape(item.name)}</b>${propertyList(item.properties)}</div></div>`).join("")
    : `<div class="tooltip-empty">no equipped items</div>`;
  return `<div class="tooltip-title">${escape(cd.name)}</div>
    <div class="tooltip-meta">$${cd.cost} · equipped value $${items.reduce((sum, { item }) => sum + item.cost, cd.cost)}</div>
    <div class="tooltip-hero"><img src="${assetHref(cd.sprite)}" alt="${escape(cd.name)}" /></div>
    ${statGrid(stats.base, stats.total)}
    ${armourTotalSectionHtml(mergedMemberCombatProperties(member))}
    <div class="tooltip-section">unit properties</div>
    ${propertyList(cd.properties || [])}
    <div class="tooltip-section">equipped items</div>
    ${itemRows}`;
}

function formationFrontAuraTooltipSection(c) {
  const m = c.applied_front_might || 0;
  const r = c.applied_front_reflexes || 0;
  const w = c.applied_front_wisdom || 0;
  const hpBonus = c.formation_hp_bonus || 0;
  if (!m && !r && !w && !hpBonus) return "";
  const parts = [];
  if (m) parts.push(`might ${signed(m)}`);
  if (r) parts.push(`reflexes ${signed(r)}`);
  if (w) parts.push(`wisdom ${signed(w)}`);
  if (hpBonus) parts.push(`max HP ${signed(hpBonus)}`);
  return `<div class="tooltip-section">formation front aura</div>
    <div class="tooltip-aura-line">${parts.join(" · ")}</div>
    <div class="tooltip-hint">From living allies that buff your front slot.</div>`;
}

function perAllyAuraTooltipSection(c) {
  const m = c.applied_per_ally_might || 0;
  const r = c.applied_per_ally_reflexes || 0;
  const w = c.applied_per_ally_wisdom || 0;
  const hpBonus = c.per_ally_hp_bonus || 0;
  if (!m && !r && !w && !hpBonus) return "";
  const parts = [];
  if (m) parts.push(`might ${signed(m)}`);
  if (r) parts.push(`reflexes ${signed(r)}`);
  if (w) parts.push(`wisdom ${signed(w)}`);
  if (hpBonus) parts.push(`max HP ${signed(hpBonus)}`);
  return `<div class="tooltip-section">per-ally aura</div>
    <div class="tooltip-aura-line">${parts.join(" · ")}</div>
    <div class="tooltip-hint">Scales with the number of other living allies.</div>`;
}

function enemyAuraTooltipSection(c) {
  const reflexDebuff = c.applied_enemy_reflex_debuff || 0;
  if (!reflexDebuff) return "";
  return `<div class="tooltip-section">active debuffs</div>
    <div class="tooltip-aura-line">reflexes ${signed(-reflexDebuff)}</div>
    <div class="tooltip-hint">From opposing living units with debuff auras.</div>`;
}

function combatantTooltip(c) {
  const cd = charDef(c.def_id);
  const effMaxHp = (c.max_hp || 0) + (c.formation_hp_bonus || 0) + (c.per_ally_hp_bonus || 0);
  const base = cd ? Object.fromEntries(STAT_KEYS.map(({ key }) => [key, cd[key] || 0])) : {
    might: c.might || 0,
    reflexes: c.reflexes || 0,
    wisdom: c.wisdom || 0,
    hp: effMaxHp,
  };
  const total = {
    might: c.might || 0,
    reflexes: Math.max(0, (c.reflexes || 0) - (c.applied_enemy_reflex_debuff || 0)),
    wisdom: c.wisdom || 0,
    hp: effMaxHp,
  };
  const itemIds = [
    ["hat", c.hat_id],
    ["left hand", c.left_hand_id],
    ["right hand", c.right_hand_id],
    ["left2", c.hand_3_id],
    ["right2", c.hand_4_id],
  ].filter(([, id]) => id);
  const itemRows = itemIds.length
    ? itemIds.map(([slot, id]) => {
      const item = itemDef(id);
      return item ? `<div class="tooltip-equipped">${itemIcon(item)}<div><b>${escape(item.name)}</b>${propertyList(item.properties)}</div></div>` : "";
    }).join("")
    : `<div class="tooltip-empty">no equipped items</div>`;
  return `<div class="tooltip-title">${escape(cd?.name || c.def_id)}</div>
    <div class="tooltip-meta">battle unit</div>
    <div class="tooltip-hero"><img src="${assetHref(c.sprite)}" alt="${escape(cd?.name || c.def_id)}" /></div>
    ${statGrid(base, total, Math.max(0, c.hp || 0))}
    ${armourTotalSectionHtml(c.properties || [])}
    ${formationFrontAuraTooltipSection(c)}
    ${perAllyAuraTooltipSection(c)}
    ${enemyAuraTooltipSection(c)}
  ${c.revive_charges ? `<div class="tooltip-section">resurrection</div><div>${escape(String(c.revive_charges))} charge${c.revive_charges === 1 ? "" : "s"} remaining</div>` : ""}
  ${c.revive_at_back_charges ? `<div class="tooltip-section">propellor revive</div><div>${escape(String(c.revive_at_back_charges))} charge${c.revive_at_back_charges === 1 ? "" : "s"} remaining</div>` : ""}
  ${(c.max_mana || 0) > 0 ? `<div class="tooltip-section">mana</div><div>${escape(String(Math.max(0, c.mana ?? 0)))} / ${escape(String(c.max_mana))}</div>` : ""}
    <div class="tooltip-section">unit properties</div>
    ${propertyList(cd?.properties || [])}
    <div class="tooltip-section">equipped items</div>
    ${itemRows}`;
}

let flashTimer = null;
let flashHideTimer = null;

function pulseHudMoney() {
  const hm = $("#hudMoney");
  if (!hm) return;
  hm.classList.remove("money-flash");
  void hm.offsetWidth;
  hm.classList.add("money-flash");
  hm.addEventListener(
    "animationend",
    () => hm.classList.remove("money-flash"),
    { once: true },
  );
}

/** @returns {string|null} */
function insufficientMoneyLine(price) {
  const bal = state.run?.money ?? 0;
  if (price <= bal) return null;
  return `need $${price - bal} more — costs $${price}, have $${bal}`;
}

/** @returns {string|null} */
function insufficientRerollLine() {
  const cost = state.consts.reroll_cost ?? 10;
  const bal = state.run?.money ?? 0;
  if (bal >= cost) return null;
  return `need $${cost - bal} more to reroll — costs $${cost}, have $${bal}`;
}

function flash(text, opts = {}) {
  const el = $("#status");
  if (!el) return;
  const { variant = "info", duration = 2400, shakeMoney = false } = opts;
  if (flashTimer) clearTimeout(flashTimer);
  if (flashHideTimer) clearTimeout(flashHideTimer);
  el.textContent = text;
  el.classList.remove("status--error", "status--info", "status--visible");
  el.classList.add(variant === "error" ? "status--error" : "status--info");
  el.setAttribute("role", variant === "error" ? "alert" : "status");
  el.setAttribute("aria-live", variant === "error" ? "assertive" : "polite");
  if (shakeMoney) pulseHudMoney();
  requestAnimationFrame(() => el.classList.add("status--visible"));
  flashTimer = setTimeout(() => {
    el.classList.remove("status--visible");
    flashHideTimer = setTimeout(() => {
      el.textContent = "";
      el.classList.remove("status--error", "status--info");
      el.setAttribute("role", "status");
      el.setAttribute("aria-live", "polite");
      flashHideTimer = null;
    }, 200);
    flashTimer = null;
  }, duration);
}

function loadLeaderboardPayload({ around = false } = {}) {
  state.lb.entries = [];
  state.lb.minPage = null;
  state.lb.maxPage = null;
  const msg = {
    type: "leaderboard",
    per_page: LB_PAGE_SIZE,
    around_player_id: state.playerId,
  };
  if (around) {
    state.lb.pendingScroll = "me";
  } else {
    state.lb.pendingScroll = "top";
    msg.page = 1;
  }
  send(msg);
}

function openLeaderboard({ around = false } = {}) {
  $("#lbModal").classList.remove("hidden");
  state.lb.open = true;
  loadLeaderboardPayload({ around });
  $("#lbCloseBtn")?.focus();
}

function closeLeaderboard() {
  $("#lbModal").classList.add("hidden");
  state.lb.open = false;
  state.lb.pendingScroll = null;
}

function loadLbPage(page, position) {
  if (!state.lb.open) return;
  if (state.lb.loading) return;
  if (page < 1 || page > state.lb.pageCount) return;
  if (state.lb.minPage !== null && page >= state.lb.minPage && page <= state.lb.maxPage) return;
  state.lb.loading = true;
  state.lb.pendingScroll = position || null;
  send({
    type: "leaderboard",
    page,
    per_page: LB_PAGE_SIZE,
    around_player_id: state.playerId,
  });
}

function handleLeaderboardMsg(msg) {
  state.lb.loading = false;
  state.lb.pageCount = msg.page_count || 1;
  state.lb.perPage = msg.per_page || LB_PAGE_SIZE;
  state.lb.playerRank = msg.player_rank ?? null;
  state.lb.playerMmr = msg.player_mmr ?? null;
  renderProfileStats();

  if (!state.lb.open) {
    return;
  }

  const startRank = (msg.page - 1) * state.lb.perPage + 1;
  const incoming = msg.entries.map((e, i) => ({ ...e, rank: startRank + i }));

  if (state.lb.minPage === null) {
    state.lb.entries = incoming;
    state.lb.minPage = msg.page;
    state.lb.maxPage = msg.page;
  } else if (msg.page === state.lb.maxPage + 1) {
    state.lb.entries = state.lb.entries.concat(incoming);
    state.lb.maxPage = msg.page;
  } else if (msg.page === state.lb.minPage - 1) {
    state.lb.entries = incoming.concat(state.lb.entries);
    state.lb.minPage = msg.page;
  } else {
    // Discontinuous (e.g. jump to "me") — reset.
    state.lb.entries = incoming;
    state.lb.minPage = msg.page;
    state.lb.maxPage = msg.page;
  }

  renderLeaderboardList();

  const scroll = $("#lbScroll");
  const pending = state.lb.pendingScroll;
  state.lb.pendingScroll = null;
  requestAnimationFrame(() => {
    if (pending === "top") {
      scroll.scrollTop = 0;
    } else if (pending === "bottom") {
      scroll.scrollTop = scroll.scrollHeight;
    } else if (pending === "me" && state.lb.playerRank != null) {
      const row = scroll.querySelector(`li[data-rank="${state.lb.playerRank}"]`);
      if (row) {
        const rowTop = row.offsetTop - scroll.offsetTop;
        scroll.scrollTop = rowTop - scroll.clientHeight / 2 + row.clientHeight / 2;
      }
    }
  });
}

function renderLeaderboardList() {
  const ol = $("#lbList");
  if (state.lb.entries.length === 0) {
    ol.innerHTML = `<li class="lb-empty">no entries yet</li>`;
  } else {
    ol.innerHTML = state.lb.entries.map((e) => {
      const isMe = e.player_id && e.player_id === state.playerId;
      const top3 = e.rank <= 3 ? ` lb-row--top${e.rank}` : "";
      return `<li class="lb-row${isMe ? " lb-row--me" : ""}${top3}" data-rank="${e.rank}">
        <span class="lb-rank">#${e.rank}</span>
        <span class="lb-name">${avatarImgHtml(e.avatar)}<span>${escape(e.name || "anon")}${isMe ? ' <span class="lb-you">you</span>' : ""}</span></span>
        <span class="lb-mmr">${e.mmr}</span>
        <span class="lb-stats">w<b>${e.wins}</b></span>
      </li>`;
    }).join("");
  }
  const topSentinel = $("#lbTopSentinel");
  if (state.lb.minPage === 1) {
    topSentinel.textContent = "";
    topSentinel.classList.add("hidden");
  } else {
    topSentinel.classList.remove("hidden");
    topSentinel.textContent = "↑ scroll for higher ranks ↑";
  }
  $("#lbBottomSentinel").classList.toggle("lb-sentinel--end", state.lb.maxPage >= state.lb.pageCount);
  if (state.lb.maxPage >= state.lb.pageCount) $("#lbBottomSentinel").textContent = "✦ end of the ladder ✦";
  else $("#lbBottomSentinel").textContent = "↓ scroll for lower ranks ↓";
  const rank = state.lb.playerRank;
  const mmr = state.run?.mmr ?? state.lb.playerMmr;
  $("#lbMeAvatar").innerHTML = avatarImgHtml(state.profile.selected_avatar || DEFAULT_AVATAR_ID, "lb-me-avatar-img");
  $("#lbMeRank").textContent = rank ? `#${rank}` : "no rank yet";
  $("#lbMeMmr").textContent = mmr != null ? `${mmr} mmr` : "—";
  $("#lbMeBtn").disabled = rank == null;
}

function populateGameOver(r, battleMsg) {
  const wins = r.wins ?? 0;
  const losses = r.losses ?? 0;
  const maxWins = state.consts.max_wins ?? 30;
  const ultimate = wins >= maxWins;

  const titleEl = $("#goTitle");
  const subEl = $("#goSubtitle");
  const recordEl = $("#goRecord");
  titleEl.classList.remove("go-title-defeat", "go-title-victory");
  if (ultimate) {
    titleEl.textContent = "ULTIMATE VICTORY";
    titleEl.classList.add("go-title-victory");
    subEl.textContent = `${wins} wins — you've topped the tournament`;
  } else {
    titleEl.textContent = "GG";
    titleEl.classList.add("go-title-defeat");
    if (battleMsg) {
      const opName = battleMsg.opponent_name || "an opponent";
      const mmrText = battleMsg.opponent_mmr_before != null
        ? ` (${Math.round(battleMsg.opponent_mmr_before)})`
        : "";
      subEl.textContent = `Defeated by ${opName}${mmrText}`;
    } else {
      subEl.textContent = "";
    }
  }
  setBgmAltActive(ultimate);
  recordEl.textContent = `record: ${wins}–${losses}`;
  $("#goWins").textContent = wins;

  const wrap = $("#goReplayWrap");
  const replayBtn = $("#goReplayBtn");
  const copyBtn = $("#copyGoReplayLinkBtn");
  if (battleMsg && battleMsg.events && battleMsg.events.length) {
    wrap.classList.remove("hidden");
    replayBtn?.classList.remove("hidden");
    copyBtn?.classList.toggle("hidden", !battleMsg.replay_id);
    renderIdentity($("#goLeftName"), r.name ?? "you", battleMsg.player_mmr_before, battleMsg.player_avatar || state.profile.selected_avatar);
    renderIdentity($("#goRightName"), battleMsg.opponent_name, battleMsg.opponent_mmr_before, battleMsg.opponent_avatar || DEFAULT_AVATAR_ID, {
      unknownMmr: battleMsg.opponent_mmr_before == null,
    });
    const key = `${r.id}:${battleMsg.events.length}`;
    if (state.gameOverReplayKey !== key) {
      state.gameOverReplayKey = key;
      playBattle($("#goReplayCanvas"), battleMsg, charDef, itemDef, () => {}, {
        showTooltip: (reference, sprite) => showTooltip(reference, combatantTooltip(sprite)),
        hideTooltipNow,
        loop: true,
      });
    }
  } else {
    wrap.classList.add("hidden");
    replayBtn?.classList.add("hidden");
    copyBtn?.classList.add("hidden");
    state.gameOverReplayKey = null;
  }
}

function renderRun() {
  const r = state.run;
  if (!r) return;
  syncRunHudValues();

  if (r.phase === "game_over") {
    if (state.battleAnimating) {
      syncRunHudVisibility();
      return;
    }
    populateGameOver(r, state.lastBattle);
    show("gameover");
    return;
  }
  if (r.phase === "battle") {
    // Stay on battle replay until "continue"; still refresh HUD numbers above.
    syncRunHudVisibility();
    return;
  }
  show("shop");
  renderTeam();
  renderShop();
}

function renderTeam() {
  const wrap = $("#teamRow");
  wrap.innerHTML = "";
  const max = state.consts.max_team || 5;
  // Visual order: team[0] is front-most. Render rightmost = front so it matches
  // combat layout (your team faces enemy on the right).
  // Team[0] is shown on the far right.

  for (let visIdx = max - 1; visIdx >= 0; visIdx--) {
    const i = visIdx; // team index
    const m = state.run.build.team[i];
    const slot = document.createElement("div");
    slot.className = "team-slot" + (m ? "" : " empty") + (i === 0 ? " front" : "");
    slot.dataset.idx = i;
    if (m) {
      const cd = charDef(m.def_id);
      slot.innerHTML = `
        <img class="portrait" src="${assetHref(cd.sprite)}" />
        <div class="name">${cd.name}</div>
        ${characterStatsHtml(cd)}
        <div class="cost">$${cd.cost}</div>
      `;
      slot.appendChild(renderItemSockets(i, m));
      if (i === 0) {
        const tag = document.createElement("div");
        tag.className = "front-tag"; tag.textContent = "FRONT";
        slot.appendChild(tag);
      }
      slot.draggable = true;
      slot.addEventListener("dragstart", onCharacterDragStart);
      slot.addEventListener("dragover", onTeamSlotDragOver);
      slot.addEventListener("dragleave", onTeamSlotDragLeave);
      slot.addEventListener("drop", onTeamSlotDrop);
      slot.addEventListener("dragend", onDragEnd);
      attachTooltip(slot, () => memberTooltip(m));
    } else {
      slot.innerHTML = `<div class="name">empty</div>`;
      slot.addEventListener("dragover", onTeamSlotDragOver);
      slot.addEventListener("dragleave", onTeamSlotDragLeave);
      slot.addEventListener("drop", onTeamSlotDrop);
    }
    slot.onclick = () => onTeamSlotClick(i, !!m);
    wrap.appendChild(slot);
  }

  wrap.classList.toggle("equip-mode", state.pendingItem !== null);
}

let dragState = null;
function setDrag(e, data) {
  dragState = data;
  e.dataTransfer.effectAllowed = "move";
  e.dataTransfer.setData("application/json", JSON.stringify(data));
  e.dataTransfer.setData("text/plain", data.type);
}
function getDrag(e) {
  if (dragState) return dragState;
  try {
    const raw = e.dataTransfer.getData("application/json");
    return raw ? JSON.parse(raw) : null;
  } catch {
    return null;
  }
}
function itemSocketId(slot) {
  if (slot === "hat") return "hat";
  return "hand";
}
function isHandSlot(slot) { return slot === "hand" || HAND_SLOT_SET.has(slot); }
function slotAccepts(targetSlot, itemSlot) {
  if (itemSlot === "hat") return targetSlot === "hat";
  if (isHandSlot(itemSlot)) return HAND_SLOT_SET.has(targetSlot);
  return false;
}
function firstFreeSlot(member, itemSlot) {
  if (!member) return null;
  if (itemSlot === "hat") return member.hat ? null : "hat";
  if (isHandSlot(itemSlot)) {
    for (const row of activeHandArmRows(member)) {
      if (!member[row.itemProp]) return row.itemSlot;
    }
    return null;
  }
  return null;
}
function canSwapTeamItem(data, target, targetSlot) {
  if (!data || data.type !== "team_item") return false;
  if (data.team === target && data.slot === targetSlot) return false;
  if (!slotAccepts(targetSlot, data.itemSlot)) return false;
  const member = state.run.build.team[target];
  const targetItemId = member?.[targetSlot];
  const targetItem = itemDef(targetItemId);
  return !!targetItem && slotAccepts(data.slot, itemSocketId(targetItem.slot));
}
function renderItemSockets(teamIdx, member) {
  const root = document.createElement("div");
  root.className = "item-sockets";

  function appendSocket(container, { itemProp, itemSlot, label }) {
    const itemId = member[itemProp];
    const socket = document.createElement("div");
    socket.className = "item-socket" + (itemId ? " filled" : "");
    socket.dataset.teamIdx = teamIdx;
    socket.dataset.itemSlot = itemSlot;
    socket.setAttribute("aria-label", label);
    const gearSlotFallback = itemProp === "hat" ? "hat" : "hand";
    if (itemId) {
      const item = itemDef(itemId);
      socket.draggable = true;
      socket.innerHTML = item ? `<img src="${assetHref(item.sprite)}" alt="${escape(item.name)}" />` : label;
      if (item) attachTooltip(socket, () => itemTooltip(item));
      socket.addEventListener("dragstart", (e) => {
        e.stopPropagation();
        hideTooltipNow();
        setDrag(e, {
          type: "team_item",
          team: teamIdx,
          slot: itemSlot,
          itemSlot: item?.slot ?? gearSlotFallback,
        });
        socket.classList.add("dragging");
      });
      socket.addEventListener("dragend", onDragEnd);
    } else {
      socket.textContent = label;
    }
    socket.addEventListener("dragover", onItemSocketDragOver);
    socket.addEventListener("dragleave", onItemSocketDragLeave);
    socket.addEventListener("drop", onItemSocketDrop);
    container.appendChild(socket);
  }

  const leftCol = document.createElement("div");
  leftCol.className = "item-socket-stack item-socket-stack--left";
  const hatCol = document.createElement("div");
  hatCol.className = "item-socket-stack item-socket-stack--hat";
  const rightCol = document.createElement("div");
  rightCol.className = "item-socket-stack item-socket-stack--right";

  const hands = activeHandArmRows(member);
  hands
    .filter((r) => r.itemProp === "left_hand" || r.itemProp === "hand_3")
    .forEach((r) => appendSocket(leftCol, r));
  appendSocket(hatCol, { itemProp: "hat", itemSlot: "hat", label: "hat" });
  hands
    .filter((r) => r.itemProp === "right_hand" || r.itemProp === "hand_4")
    .forEach((r) => appendSocket(rightCol, r));

  root.append(hatCol, leftCol, rightCol);
  return root;
}
function onCharacterDragStart(e) {
  hideTooltipNow();
  const from = parseInt(e.currentTarget.dataset.idx, 10);
  setDrag(e, { type: "character", team: from });
  e.currentTarget.classList.add("dragging");
}
function onTeamSlotDragOver(e) {
  const data = getDrag(e);
  if (!data) return;
  const to = parseInt(e.currentTarget.dataset.idx, 10);
  if (data.type === "shop_character") {
    const max = state.consts.max_team || 8;
    const team = state.run.build.team;
    if (team.length >= max || Number.isNaN(to)) return;
    const hasMember = !!team[to];
    if (!hasMember && to < team.length) return;
    e.preventDefault();
    e.dataTransfer.dropEffect = "move";
    e.currentTarget.classList.add("drag-over");
    return;
  }
  if (data.type === "character") {
    if (data.team === to || Number.isNaN(to)) return;
    if (!state.run.build.team[data.team] || !state.run.build.team[to]) return;
    e.preventDefault();
    e.dataTransfer.dropEffect = "move";
    e.currentTarget.classList.add("drag-over");
    return;
  }
  e.preventDefault();
  e.dataTransfer.dropEffect = "move";
  e.currentTarget.classList.add("drag-over");
}
function onTeamSlotDragLeave(e) { e.currentTarget.classList.remove("drag-over"); }
function onTeamSlotDrop(e) {
  e.preventDefault();
  const data = getDrag(e);
  const to = parseInt(e.currentTarget.dataset.idx, 10);
  e.currentTarget.classList.remove("drag-over");
  if (!data || Number.isNaN(to)) return;
  if (data.type === "character") {
    if (data.team === to) return;
    if (!state.run.build.team[data.team] || !state.run.build.team[to]) return;
    send({ type: "reorder", from: data.team, to });
  } else if (data.type === "shop_character") {
    const max = state.consts.max_team || 8;
    const team = state.run.build.team;
    if (team.length >= max) {
      flash("team full", { variant: "info" });
      return;
    }
    const hasMember = !!team[to];
    const insertAt = hasMember ? to : team.length;
    const cid = state.run.shop.characters[data.slot];
    const cd = charDef(cid);
    const line = cd ? insufficientMoneyLine(cd.cost) : null;
    if (line) {
      flash(line, { variant: "error", shakeMoney: true, duration: 3600 });
      return;
    }
    send({ type: "buy_character", slot: data.slot, target: insertAt });
  } else if (data.type === "shop_item") {
    if (!state.run.build.team[to]) return;
    const sid = state.run.shop.items[data.slot];
    const def = itemDef(sid);
    const line = def ? insufficientMoneyLine(def.cost) : null;
    if (line) {
      flash(line, { variant: "error", shakeMoney: true, duration: 3600 });
      return;
    }
    send({ type: "buy_item", slot: data.slot, target: to });
  } else if (data.type === "team_item") {
    const targetSlot = firstFreeSlot(state.run.build.team[to], data.itemSlot);
    if (!targetSlot) { flash("no open socket", { variant: "info" }); return; }
    send({ type: "move_item", from_team: data.team, from_slot: data.slot, to_team: to, to_slot: targetSlot });
  }
}
function onDragEnd(e) {
  hideTooltipNow();
  e.currentTarget.classList.remove("dragging");
  document.querySelectorAll(".drag-over").forEach(el => el.classList.remove("drag-over"));
  dragState = null;
}
function onItemSocketDragOver(e) {
  const data = getDrag(e);
  if (!data || (data.type !== "shop_item" && data.type !== "team_item")) return;
  const targetSlot = e.currentTarget.dataset.itemSlot;
  const target = parseInt(e.currentTarget.dataset.teamIdx, 10);
  const member = state.run.build.team[target];
  if (!member) return;
  if (data.type === "team_item" && data.team === target && data.slot === targetSlot) return;
  const filled = e.currentTarget.classList.contains("filled");
  const destOk =
    (slotAccepts(targetSlot, data.itemSlot) && !filled) ||
    canSwapTeamItem(data, target, targetSlot) ||
    firstFreeSlot(member, data.itemSlot);
  if (!destOk) return;
  e.preventDefault();
  e.stopPropagation();
  e.dataTransfer.dropEffect = "move";
  e.currentTarget.classList.add("drag-over");
}
function onItemSocketDragLeave(e) { e.currentTarget.classList.remove("drag-over"); }
function onItemSocketDrop(e) {
  const data = getDrag(e);
  const target = parseInt(e.currentTarget.dataset.teamIdx, 10);
  const targetSlot = e.currentTarget.dataset.itemSlot;
  e.currentTarget.classList.remove("drag-over");
  if (!data || (data.type !== "shop_item" && data.type !== "team_item")) return;

  const member = state.run.build.team[target];
  if (!member) return;
  if (data.type === "team_item" && data.team === target && data.slot === targetSlot) {
    e.preventDefault();
    e.stopPropagation();
    return;
  }

  const filled = e.currentTarget.classList.contains("filled");
  const canSwap = canSwapTeamItem(data, target, targetSlot);
  let destSlot =
    (slotAccepts(targetSlot, data.itemSlot) && (!filled || canSwap))
      ? targetSlot
      : firstFreeSlot(member, data.itemSlot);

  if (!destSlot) {
    if (filled && slotAccepts(targetSlot, data.itemSlot)) {
      flash("item socket taken", { variant: "info" });
    } else {
      flash("no open socket", { variant: "info" });
    }
    e.preventDefault();
    e.stopPropagation();
    return;
  }

  if (data.type === "team_item" && data.team === target && data.slot === destSlot) {
    e.preventDefault();
    e.stopPropagation();
    return;
  }

  e.preventDefault();
  e.stopPropagation();

  if (data.type === "shop_item") {
    const sid = state.run.shop.items[data.slot];
    const def = itemDef(sid);
    const line = def ? insufficientMoneyLine(def.cost) : null;
    if (line) {
      flash(line, { variant: "error", shakeMoney: true, duration: 3600 });
      return;
    }
    send({ type: "buy_item_to_slot", slot: data.slot, target, target_slot: destSlot });
  } else if (data.type === "team_item") {
    send({ type: "move_item", from_team: data.team, from_slot: data.slot, to_team: target, to_slot: destSlot });
  }
}

function renderShop() {
  const bal = state.run.money;
  const sc = $("#shopChars"); sc.innerHTML = "";
  state.run.shop.characters.forEach((id, i) => {
    if (!id) { sc.appendChild(emptyCard()); return; }
    const cd = charDef(id);
    const c = document.createElement("div");
    const cantAfford = cd.cost > bal;
    c.className = "card" + (cantAfford ? " cant-afford" : "");
    c.innerHTML = `
      <img src="${assetHref(cd.sprite)}" />
      <div class="name">${cd.name}</div>
      ${characterStatsHtml(cd)}
      <div class="cost">$${cd.cost}</div>
    `;
    attachTooltip(c, () => characterTooltip(cd));
    c.draggable = !cantAfford;
    c.addEventListener("dragstart", (e) => {
      hideTooltipNow();
      setDrag(e, { type: "shop_character", slot: i });
      c.classList.add("dragging");
    });
    c.addEventListener("dragend", onDragEnd);
    c.onclick = () => {
      if (cantAfford) {
        flash(insufficientMoneyLine(cd.cost), {
          variant: "error",
          shakeMoney: true,
          duration: 3600,
        });
        return;
      }
      send({ type: "buy_character", slot: i, target: state.run.build.team.length });
    };
    sc.appendChild(c);
  });
  const si = $("#shopItems"); si.innerHTML = "";
  state.run.shop.items.forEach((id, i) => {
    if (!id) { const e = emptyCard(); e.classList.add("item-card"); si.appendChild(e); return; }
    const it = itemDef(id);
    const c = document.createElement("div");
    const cantAfford = it.cost > bal;
    c.className =
      "card item-card" +
      (state.pendingItem === i ? " equip-mode" : "") +
      (cantAfford ? " cant-afford" : "");
    c.draggable = !cantAfford;
    c.innerHTML = `
      <img src="${assetHref(it.sprite)}" />
      <div class="name">${it.name}</div>
      <div class="cost">$${it.cost}</div>
      <div class="stats">${it.slot}</div>
    `;
    c.addEventListener("dragstart", (e) => {
      hideTooltipNow();
      setDrag(e, { type: "shop_item", slot: i, itemSlot: itemSocketId(it.slot) });
      c.classList.add("dragging");
    });
    c.addEventListener("dragend", onDragEnd);
    attachTooltip(c, () => itemTooltip(it));
    c.onclick = () => {
      state.pendingItem = state.pendingItem === i ? null : i;
      renderTeam(); renderShop();
    };
    si.appendChild(c);
  });
}

const emptyCard = () => { const e = document.createElement("div"); e.className="card empty"; e.innerHTML="<div class='name'>—</div>"; return e; };

function onTeamSlotClick(idx, hasMember) {
  if (state.pendingItem !== null) {
    if (!hasMember) {
      flash("equip onto a character", { variant: "info" });
      return;
    }
    const sid = state.run.shop.items[state.pendingItem];
    const def = itemDef(sid);
    const line = def ? insufficientMoneyLine(def.cost) : null;
    if (line) {
      flash(line, { variant: "error", shakeMoney: true, duration: 3600 });
      state.pendingItem = null;
      renderTeam();
      renderShop();
      return;
    }
    send({ type: "buy_item", slot: state.pendingItem, target: idx });
    state.pendingItem = null;
    return;
  }
  // Selling is now drag-to-sell only (no click-confirm).
}

function wireInventoryDrop() {
  const z = $("#shopInventory"); if (!z || z.dataset.wired) return;
  z.dataset.wired = "1";
  z.addEventListener("dragover", (e) => {
    const data = getDrag(e);
    if (!data || (data.type !== "character" && data.type !== "team_item")) return;
    e.preventDefault();
    e.dataTransfer.dropEffect = "move";
    z.classList.add("drag-over");
  });
  z.addEventListener("dragleave", () => z.classList.remove("drag-over"));
  z.addEventListener("drop", (e) => {
    e.preventDefault();
    const data = getDrag(e);
    z.classList.remove("drag-over");
    if (!data) return;
    if (data.type === "character") {
      send({ type: "sell", team_index: data.team });
    } else if (data.type === "team_item") {
      send({ type: "sell_item", team_index: data.team, item_slot: data.slot });
    }
    dragState = null;
  });
}

// Wire UI
state.profile.player_id = state.playerId;
renderProfilePill();
ensureBgmAudio();
$("#bgmMuteBtn").onclick = () => setBgmMuted(!(state.bgmMuted || state.bgmVolume <= 0));
$("#bgmVolume").addEventListener("input", (e) => setBgmVolume(e.currentTarget.value));
window.addEventListener("pointerdown", startBgm, { once: true });
window.addEventListener("keydown", startBgm, { once: true });
$("#profilePill").onclick = openProfileModal;
$("#helpBtn").onclick = openHelpModal;
$("#profileCloseBtn").onclick = closeProfileModal;
$("#helpCloseBtn").onclick = closeHelpModal;
$("#saveProfileBtn").onclick = saveProfile;
$("#profileNameInput").addEventListener("keydown", (e) => {
  if (e.key === "Enter") {
    saveProfile();
  }
});
$("#newRunBtn").onclick = () => {
  startNewRun();
};
$("#openLbBtn").onclick = () => openLeaderboard();
$("#lbCloseBtn").onclick = closeLeaderboard;
$("#lbModal").addEventListener("click", (e) => {
  if (e.target.matches("[data-close-lb-modal]")) closeLeaderboard();
});
$("#profileModal").addEventListener("click", (e) => {
  if (e.target.matches("[data-close-profile-modal]")) closeProfileModal();
});
$("#helpModal").addEventListener("click", (e) => {
  if (e.target.matches("[data-close-help-modal]")) closeHelpModal();
});
$("#lbTopBtn").onclick = () => openLeaderboard({ around: false });
$("#lbMeBtn").onclick = () => openLeaderboard({ around: true });
$("#lbUpBtn").onclick = () => {
  if (state.lb.minPage > 1) loadLbPage(state.lb.minPage - 1, "top");
  else $("#lbScroll").scrollTop = 0;
};
$("#lbDownBtn").onclick = () => {
  if (state.lb.maxPage < state.lb.pageCount) loadLbPage(state.lb.maxPage + 1, "bottom");
  else $("#lbScroll").scrollTop = $("#lbScroll").scrollHeight;
};
{
  const scroll = $("#lbScroll");
  scroll.addEventListener("scroll", () => {
    if (!state.lb.open || state.lb.loading) return;
    const nearTop = scroll.scrollTop < 80;
    const nearBottom = scroll.scrollHeight - scroll.scrollTop - scroll.clientHeight < 80;
    if (nearBottom && state.lb.maxPage < state.lb.pageCount) {
      loadLbPage(state.lb.maxPage + 1);
    } else if (nearTop && state.lb.minPage > 1) {
      const before = scroll.scrollHeight;
      loadLbPage(state.lb.minPage - 1);
      // After prepend, restore relative scroll position so view doesn't jump.
      const obs = new MutationObserver(() => {
        scroll.scrollTop += scroll.scrollHeight - before;
        obs.disconnect();
      });
      obs.observe($("#lbList"), { childList: true });
    }
  });
}
$("#rerollBtn").onclick = () => {
  const line = insufficientRerollLine();
  if (line) {
    flash(line, { variant: "error", shakeMoney: true, duration: 3600 });
    return;
  }
  send({ type: "reroll" });
};
$("#battleBtn").onclick = () => send({ type: "battle" });
$("#quitRunBtn").onclick = openQuitRunModal;
$("#cancelQuitRunBtn").onclick = closeQuitRunModal;
$("#confirmQuitRunBtn").onclick = () => {
  closeQuitRunModal();
  startNewRun();
};
$("#quitRunModal").addEventListener("click", (e) => {
  if (e.target.matches("[data-close-quit-modal]")) closeQuitRunModal();
});
window.addEventListener("keydown", (e) => {
  if (e.key === "Escape" && !$("#quitRunModal").classList.contains("hidden")) {
    closeQuitRunModal();
  } else if (e.key === "Escape" && !$("#helpModal").classList.contains("hidden")) {
    closeHelpModal();
  } else if (e.key === "Escape" && !$("#profileModal").classList.contains("hidden")) {
    closeProfileModal();
  } else if (e.key === "Escape" && state.lb.open) {
    closeLeaderboard();
  }
});
$("#nextRoundBtn").onclick = () => {
  if (isSharedReplayActive()) {
    leaveSharedReplayMode();
    show("start");
    if (state.auth.edgeCase) {
      renderStartScreen();
    } else {
      resumeRun(true);
    }
    return;
  }
  renderRun();
};
$("#replayBattleBtn").onclick = () => {
  if (!state.lastBattle || state.battleAnimating) return;
  $("#battleCanvas").__battleTooltipCleanup?.();
  $("#battleLog").innerHTML = "";
  state.battleAnimating = true;
  $("#nextRoundBtn").classList.add("hidden");
  $("#replayBattleBtn").classList.add("hidden");
  syncReplayLinkButtons();
  const msg = state.lastBattle;
  playBattle($("#battleCanvas"), msg, charDef, itemDef, () => {
    if (isSharedReplayActive()) {
      finishSharedReplayPlayback(msg);
      return;
    }
    state.battleAnimating = false;
    if (state.run?.phase !== "game_over") {
      $("#nextRoundBtn").classList.remove("hidden");
      $("#replayBattleBtn").classList.remove("hidden");
      syncReplayLinkButtons();
    }
  }, {
    showTooltip: (reference, sprite) => showTooltip(reference, combatantTooltip(sprite)),
    hideTooltipNow,
  });
};
$("#copyReplayLinkBtn").onclick = () => {
  copyCurrentReplayLink();
};
$("#goReplayBtn").onclick = () => {
  if (!state.lastBattle) return;
  stopGameOverReplay();
  const r = state.run;
  if (!r) return;
  state.gameOverReplayKey = `${r.id}:${state.lastBattle.events.length}`;
  playBattle($("#goReplayCanvas"), state.lastBattle, charDef, itemDef, () => {}, {
    showTooltip: (reference, sprite) => showTooltip(reference, combatantTooltip(sprite)),
    hideTooltipNow,
    loop: true,
  });
};
$("#copyGoReplayLinkBtn").onclick = () => {
  copyCurrentReplayLink();
};
$("#goRestart").onclick = () => {
  leaveSharedReplayMode();
  state.lastBattle = null;
  state.battleAnimating = false;
  startNewRun();
};

wireInventoryDrop();

// ---- auth wiring ----
$("#loginBtn")?.addEventListener("click", showLoginScreen);
$("#loginBackBtn")?.addEventListener("click", () => {
  $("#loginError").textContent = "";
  show("start");
});
$("#loginForm")?.addEventListener("submit", (e) => {
  e.preventDefault();
  handleLoginSubmit({
    username: $("#loginUsername").value.trim(),
    password: $("#loginPassword").value,
    stay: $("#loginStay").checked,
    errEl: $("#loginError"),
    onSuccess: () => show("start"),
  });
});
$("#edgeCaseLoginForm")?.addEventListener("submit", (e) => {
  e.preventDefault();
  handleLoginSubmit({
    username: state.auth.username || "",
    password: $("#edgeCasePassword").value,
    stay: $("#edgeCaseStay").checked,
    errEl: $("#edgeCaseError"),
    onSuccess: () => show("start"),
  });
});
$("#signInAsOtherBtn")?.addEventListener("click", signInAsAnotherUser);
$("#registerBtn")?.addEventListener("click", openRegisterPanel);
$("#registerCancelBtn")?.addEventListener("click", closeRegisterPanel);
$("#registerForm")?.addEventListener("submit", handleRegisterSubmit);
$("#logoutBtn")?.addEventListener("click", async () => {
  closeProfileModal();
  await handleLogout();
  show("start");
});

show("start");

(async function boot() {
  const w = await fetchWhoami();
  applyWhoami(w);
  renderProfileAccountControls();
  renderStartScreen();
  connect();
})();
