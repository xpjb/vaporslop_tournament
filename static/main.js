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

const state = {
  ws: null,
  defs: { characters: [], items: [] },
  consts: {},
  run: null,
  pendingItem: null, // shop item slot waiting for team target
  lastBattle: null,
  battleAnimating: false,
  autoResumePending: false,
  playerId: getOrCreatePlayerId(),
  nickname: localStorage.getItem(NICKNAME_KEY) || "anon",
  bgmAudio: null,
  bgmVolume: getStoredBgmVolume(),
  bgmMuted: localStorage.getItem(BGM_MUTED_KEY) === "1",
  lb: {
    entries: [], // contiguous list of {rank, player_id, name, mmr, wins}
    minPage: null,
    maxPage: null,
    pageCount: 1,
    perPage: LB_PAGE_SIZE,
    playerRank: null,
    loading: false,
    pendingScroll: null, // "top" | "rank:<n>" | null
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

function syncBgmControls() {
  const audio = state.bgmAudio;
  if (audio) {
    audio.volume = state.bgmVolume;
    audio.muted = state.bgmMuted || state.bgmVolume <= 0;
  }
  const slider = $("#bgmVolume");
  if (slider) slider.value = String(Math.round(state.bgmVolume * 100));
  const btn = $("#bgmMuteBtn");
  if (!btn) return;
  const muted = state.bgmMuted || state.bgmVolume <= 0;
  btn.textContent = muted ? "🔇" : "🔊";
  btn.setAttribute("aria-pressed", muted ? "true" : "false");
  btn.setAttribute("aria-label", muted ? "unmute background music" : "mute background music");
}

function startBgm() {
  if (state.bgmMuted || state.bgmVolume <= 0) return;
  const audio = ensureBgmAudio();
  syncBgmControls();
  audio.play().catch(() => {});
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
  localStorage.setItem(NICKNAME_KEY, next);
  $("#nameInput").value = next;
  $("#hudNameInput").value = next;
  if (state.run) state.run.name = next;
  if (notifyServer && state.run) send({ type: "rename_player", name: next });
}

function show(screenId) {
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
  if (document.activeElement !== $("#hudNameInput")) {
    $("#hudNameInput").value = r.name;
  }
  $("#hudMoney").textContent = r.money;
  $("#hudWins").textContent = r.wins;
  $("#hudLosses").textContent = r.losses;
  $("#hudMmr").textContent = r.mmr ?? "????";
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
  setNickname($("#nameInput").value || state.nickname, false);
  state.lastBattle = null;
  state.battleAnimating = false;
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
      state.consts = msg.constants;
      $("#hudMaxLosses").textContent = msg.constants.max_losses;
      $("#hudMaxWins").textContent = msg.constants.max_wins;
      $("#goMaxWins").textContent = msg.constants.max_wins;
      $("#rerollCost").textContent = msg.constants.reroll_cost;
      if (msg.site_stats) syncSiteStats(msg.site_stats);
      resumeRun(true);
      loadLeaderboard();
      break;
    case "state":
      state.autoResumePending = false;
      state.run = msg.run;
      setNickname(msg.run.name, false);
      renderRun();
      break;
    case "battle":
      if (state.run && !isCurrentRunId(msg.run_id)) break;
      state.lastBattle = msg;
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
      $("#leftName").textContent = formatPlayerName(state.run?.name ?? "you", msg.player_mmr_before);
      $("#rightName").textContent = formatPlayerName(msg.opponent_name, msg.opponent_mmr_before, {
        unknownMmr: msg.opponent_mmr_before == null,
      });
      $("#nextRoundBtn").classList.add("hidden");
      $("#battleLog").innerHTML = "";
      state.battleAnimating = true;
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
        } else {
          renderRun();
        }
      }, {
        showTooltip: (reference, sprite) => showTooltip(reference, combatantTooltip(sprite)),
        hideTooltip,
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
const charDef = (id) => state.defs.characters.find(c => c.id === id);
const itemDef = (id) => state.defs.items.find(i => i.id === id);
const SOCKETS = [
  { key: "hat", label: "hat" },
  { key: "left_hand", label: "left" },
  { key: "right_hand", label: "right" },
];
const STAT_KEYS = [
  { key: "might", label: "might" },
  { key: "reflexes", label: "reflexes" },
  { key: "wisdom", label: "wisdom" },
  { key: "hp", label: "hp" },
];
const SLOT_LABELS = { hat: "hat", left_hand: "left hand", right_hand: "right hand" };

let tooltipEl = null;
let cleanupTooltip = null;
let activeTooltipReference = null;
let activeTooltipHtml = "";

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
  if (cleanupTooltip) cleanupTooltip();
  cleanupTooltip = null;
  activeTooltipReference = null;
  activeTooltipHtml = "";
  ensureTooltip().classList.add("hidden");
}

function attachTooltip(el, content) {
  const show = () => showTooltip(el, typeof content === "function" ? content() : content);
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
  return SOCKETS
    .map(({ key }) => ({ key, item: member?.[key] ? itemDef(member[key]) : null }))
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
      const mod = delta ? `<span class="tooltip-delta">${signed(delta)}</span>` : "";
      const baseText = delta ? `<span class="tooltip-base">base ${base[key]}</span>` : "";
      return `<div><span>${label}</span><b>${value}</b>${mod}${baseText}</div>`;
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
    ? items.map(({ key, item }) => `<div class="tooltip-equipped">${itemIcon(item, SLOT_LABELS[key])}<div><b>${escape(item.name)}</b>${propertyList(item.properties)}</div></div>`).join("")
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

function combatantTooltip(c) {
  const cd = charDef(c.def_id);
  const effMaxHp = (c.max_hp || 0) + (c.formation_hp_bonus || 0);
  const base = cd ? Object.fromEntries(STAT_KEYS.map(({ key }) => [key, cd[key] || 0])) : {
    might: c.might || 0,
    reflexes: c.reflexes || 0,
    wisdom: c.wisdom || 0,
    hp: effMaxHp,
  };
  const total = {
    might: c.might || 0,
    reflexes: c.reflexes || 0,
    wisdom: c.wisdom || 0,
    hp: effMaxHp,
  };
  const itemIds = [
    ["hat", c.hat_id],
    ["left hand", c.left_hand_id],
    ["right hand", c.right_hand_id],
  ].filter(([, id]) => id);
  const itemRows = itemIds.length
    ? itemIds.map(([slot, id]) => {
      const item = itemDef(id);
      return item ? `<div class="tooltip-equipped">${itemIcon(item, slot)}<div><b>${escape(item.name)}</b>${propertyList(item.properties)}</div></div>` : "";
    }).join("")
    : `<div class="tooltip-empty">no equipped items</div>`;
  return `<div class="tooltip-title">${escape(cd?.name || c.def_id)}</div>
    <div class="tooltip-meta">battle unit</div>
    <div class="tooltip-hero"><img src="${assetHref(c.sprite)}" alt="${escape(cd?.name || c.def_id)}" /></div>
    ${statGrid(base, total, Math.max(0, c.hp || 0))}
    ${armourTotalSectionHtml(c.properties || [])}
    ${formationFrontAuraTooltipSection(c)}
  ${c.revive_charges ? `<div class="tooltip-section">resurrection</div><div>${escape(String(c.revive_charges))} charge${c.revive_charges === 1 ? "" : "s"} remaining</div>` : ""}
  ${(c.max_mana || 0) > 0 ? `<div class="tooltip-section">mana</div><div>${escape(String(Math.max(0, c.mana ?? 0)))} / ${escape(String(c.max_mana))}</div>` : ""}
    <div class="tooltip-section">unit properties</div>
    ${propertyList(cd?.properties || [])}
    <div class="tooltip-section">equipped items</div>
    ${itemRows}`;
}

let flashTimer = null;

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
  el.textContent = text;
  el.classList.remove("status--error", "status--info");
  el.classList.add(variant === "error" ? "status--error" : "status--info");
  el.setAttribute("role", variant === "error" ? "alert" : "status");
  el.setAttribute("aria-live", variant === "error" ? "assertive" : "polite");
  if (shakeMoney) pulseHudMoney();
  if (flashTimer) clearTimeout(flashTimer);
  flashTimer = setTimeout(() => {
    el.textContent = "";
    el.classList.remove("status--error", "status--info");
    el.setAttribute("role", "status");
    el.setAttribute("aria-live", "polite");
    flashTimer = null;
  }, duration);
}

function loadLeaderboard({ around = false } = {}) {
  state.lb.entries = [];
  state.lb.minPage = null;
  state.lb.maxPage = null;
  if (around) {
    state.lb.pendingScroll = "me";
    send({
      type: "leaderboard",
      per_page: LB_PAGE_SIZE,
      around_player_id: state.playerId,
    });
  } else {
    state.lb.pendingScroll = "top";
    send({ type: "leaderboard", page: 1, per_page: LB_PAGE_SIZE });
  }
}

function loadLbPage(page, position) {
  if (state.lb.loading) return;
  if (page < 1 || page > state.lb.pageCount) return;
  if (state.lb.minPage !== null && page >= state.lb.minPage && page <= state.lb.maxPage) return;
  state.lb.loading = true;
  state.lb.pendingScroll = position || null;
  send({ type: "leaderboard", page, per_page: LB_PAGE_SIZE });
}

function handleLeaderboardMsg(msg) {
  state.lb.loading = false;
  state.lb.pageCount = msg.page_count || 1;
  state.lb.perPage = msg.per_page || LB_PAGE_SIZE;
  if (msg.player_rank != null) state.lb.playerRank = msg.player_rank;

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
        <span class="lb-name">${escape(e.name || "anon")}${isMe ? ' <span class="lb-you">you</span>' : ""}</span>
        <span class="lb-mmr">${e.mmr}</span>
        <span class="lb-stats">w<b>${e.wins}</b></span>
      </li>`;
    }).join("");
  }
  $("#lbTopSentinel").classList.toggle("lb-sentinel--end", state.lb.minPage === 1);
  $("#lbBottomSentinel").classList.toggle("lb-sentinel--end", state.lb.maxPage >= state.lb.pageCount);
  if (state.lb.minPage === 1) $("#lbTopSentinel").textContent = "✦ top of the ladder ✦";
  else $("#lbTopSentinel").textContent = "↑ scroll for higher ranks ↑";
  if (state.lb.maxPage >= state.lb.pageCount) $("#lbBottomSentinel").textContent = "✦ end of the ladder ✦";
  else $("#lbBottomSentinel").textContent = "↓ scroll for lower ranks ↓";
  const info = state.lb.playerRank
    ? `your rank · #${state.lb.playerRank}`
    : "no rank yet — finish a run";
  $("#lbRankInfo").textContent = info;
  $("#lbMeBtn").disabled = state.lb.playerRank == null;
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
    $("#goWins").textContent = r.wins;
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
        <div class="stats">⚔${cd.might} ⚡${cd.reflexes} ✦${cd.wisdom} ❤${cd.hp}</div>
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
function slotAccepts(targetSlot, itemSlot) {
  if (itemSlot === "hat") return targetSlot === "hat";
  if (itemSlot === "hand") return targetSlot === "left_hand" || targetSlot === "right_hand";
  if (itemSlot === "left_hand" || itemSlot === "right_hand") {
    return targetSlot === "left_hand" || targetSlot === "right_hand";
  }
  return false;
}
function firstFreeSlot(member, itemSlot) {
  if (!member) return null;
  if (itemSlot === "hat") return member.hat ? null : "hat";
  if (itemSlot === "hand" || itemSlot === "left_hand" || itemSlot === "right_hand") {
    if (!member.left_hand) return "left_hand";
    if (!member.right_hand) return "right_hand";
    return null;
  }
  return null;
}
function renderItemSockets(teamIdx, member) {
  const sockets = document.createElement("div");
  sockets.className = "item-sockets";
  SOCKETS.forEach(({ key, label }) => {
    const itemId = member[key];
    const socket = document.createElement("div");
    socket.className = "item-socket" + (itemId ? " filled" : "");
    socket.dataset.teamIdx = teamIdx;
    socket.dataset.itemSlot = key;
    socket.setAttribute("aria-label", label);
    if (itemId) {
      const item = itemDef(itemId);
      socket.draggable = true;
      socket.innerHTML = item ? `<img src="${assetHref(item.sprite)}" alt="${escape(item.name)}" />` : label;
      if (item) attachTooltip(socket, () => itemTooltip(item));
      socket.addEventListener("dragstart", (e) => {
        e.stopPropagation();
        hideTooltip();
        setDrag(e, { type: "team_item", team: teamIdx, slot: key, itemSlot: item?.slot ?? key });
        socket.classList.add("dragging");
      });
      socket.addEventListener("dragend", onDragEnd);
    } else {
      socket.textContent = label;
    }
    socket.addEventListener("dragover", onItemSocketDragOver);
    socket.addEventListener("dragleave", onItemSocketDragLeave);
    socket.addEventListener("drop", onItemSocketDrop);
    sockets.appendChild(socket);
  });
  return sockets;
}
function onCharacterDragStart(e) {
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
  const filled = e.currentTarget.classList.contains("filled");
  const destOk =
    (slotAccepts(targetSlot, data.itemSlot) && !filled) ||
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

  const filled = e.currentTarget.classList.contains("filled");
  let destSlot =
    slotAccepts(targetSlot, data.itemSlot) && !filled
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
      <div class="stats">⚔${cd.might} ⚡${cd.reflexes} ✦${cd.wisdom} ❤${cd.hp}</div>
      <div class="cost">$${cd.cost}</div>
    `;
    attachTooltip(c, () => characterTooltip(cd));
    c.draggable = !cantAfford;
    c.addEventListener("dragstart", (e) => {
      hideTooltip();
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
      hideTooltip();
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
$("#nameInput").value = state.nickname;
$("#hudNameInput").value = state.nickname;
ensureBgmAudio();
$("#bgmMuteBtn").onclick = () => setBgmMuted(!(state.bgmMuted || state.bgmVolume <= 0));
$("#bgmVolume").addEventListener("input", (e) => setBgmVolume(e.currentTarget.value));
window.addEventListener("pointerdown", startBgm, { once: true });
window.addEventListener("keydown", startBgm, { once: true });
$("#saveStartNicknameBtn").onclick = () => {
  setNickname($("#nameInput").value);
  flash("nickname saved", { variant: "info" });
};
$("#saveNicknameBtn").onclick = () => {
  setNickname($("#hudNameInput").value);
  flash("nickname saved", { variant: "info" });
};
$("#nameInput").addEventListener("change", () => setNickname($("#nameInput").value, false));
$("#hudNameInput").addEventListener("keydown", (e) => {
  if (e.key === "Enter") {
    setNickname($("#hudNameInput").value);
    flash("nickname saved", { variant: "info" });
  }
});
$("#newRunBtn").onclick = () => {
  startNewRun();
};
$("#lbTopBtn").onclick = () => loadLeaderboard({ around: false });
$("#lbMeBtn").onclick = () => loadLeaderboard({ around: true });
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
    if (state.lb.loading) return;
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
  }
});
$("#nextRoundBtn").onclick = () => renderRun();
$("#goRestart").onclick = () => {
  state.run = null;
  state.lastBattle = null;
  state.battleAnimating = false;
  show("start");
};

wireInventoryDrop();
show("start");
connect();
