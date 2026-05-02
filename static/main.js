import { computePosition, autoUpdate, offset, flip, shift } from "https://cdn.jsdelivr.net/npm/@floating-ui/dom/+esm";
import { playBattle } from "/render.js";

const $ = (sel) => document.querySelector(sel);
const $$ = (sel) => Array.from(document.querySelectorAll(sel));

const PLAYER_ID_KEY = "playerUuid";
const NICKNAME_KEY = "playerNickname";
const RUN_ID_KEY = "runId";
const LB_PAGE_SIZE = 10;

const state = {
  ws: null,
  defs: { characters: [], items: [] },
  consts: {},
  run: null,
  pendingItem: null, // shop item slot waiting for team target
  lastBattle: null,
  battleAnimating: false,
  playerId: getOrCreatePlayerId(),
  nickname: localStorage.getItem(NICKNAME_KEY) || "anon",
  leaderboardPage: 1,
  leaderboardPageCount: 1,
};

function getOrCreatePlayerId() {
  let id = localStorage.getItem(PLAYER_ID_KEY);
  if (id) return id;
  id = crypto.randomUUID ? crypto.randomUUID() : `${Date.now()}-${Math.random().toString(16).slice(2)}`;
  localStorage.setItem(PLAYER_ID_KEY, id);
  return id;
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

function connect() {
  const proto = location.protocol === "https:" ? "wss" : "ws";
  state.ws = new WebSocket(`${proto}://${location.host}/ws`);
  state.ws.onmessage = (ev) => {
    const msg = JSON.parse(ev.data);
    handleServer(msg);
  };
  state.ws.onclose = () => setTimeout(connect, 1000);
}
function send(obj) { state.ws?.send(JSON.stringify(obj)); }

function handleServer(msg) {
  switch (msg.type) {
    case "defs":
      state.defs.characters = msg.characters;
      state.defs.items = msg.items;
      state.consts = msg.constants;
      $("#hudMaxLosses").textContent = msg.constants.max_losses;
      $("#rerollCost").textContent = msg.constants.reroll_cost;
      break;
    case "state":
      state.run = msg.run;
      localStorage.setItem(RUN_ID_KEY, msg.run.id);
      setNickname(msg.run.name, false);
      renderRun();
      break;
    case "battle":
      state.lastBattle = msg;
      // Authoritative post-fight snapshot (same payload the server persists right after).
      if (state.run) {
        Object.assign(state.run, {
          phase: msg.phase,
          wins: msg.wins,
          losses: msg.losses,
          streak: msg.streak,
          alive: msg.alive,
          money: msg.money_after,
        });
      }
      show("battle");
      $("#leftName").textContent = state.run?.name ?? "you";
      $("#rightName").textContent = msg.opponent_name;
      $("#nextRoundBtn").classList.add("hidden");
      $("#battleLog").innerHTML = "";
      state.battleAnimating = true;
      playBattle($("#battleCanvas"), msg, charDef, itemDef, () => {
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
          Object.assign(state.run, {
            phase: m.phase,
            wins: m.wins,
            losses: m.losses,
            streak: m.streak,
            alive: m.alive,
            money: m.money_after,
          });
        }
        if (state.run?.phase !== "game_over") {
          $("#nextRoundBtn").classList.remove("hidden");
        }
        renderRun();
      }, {
        showTooltip: (reference, sprite) => showTooltip(reference, combatantTooltip(sprite)),
        hideTooltip,
      });
      break;
    case "leaderboard":
      show("leaderboard");
      renderLeaderboard(msg);
      break;
    case "error": {
      const m = msg.message;
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
    case "healer": return "heals allies for wisdom";
    case "freeze_on_hit": return "freezes on hit";
    case "summon_on_enemy_death": return `summon ${escape(p.species)} when an enemy dies`;
    case "summon_on_ally_death": return `summon ${escape(p.species)} when an ally dies`;
    case "might_on_ally_death": return (
      `when an ally dies: might ${signed(p.might)} for this battle`
    );
    case "crit_strike": return `${escape(String(p.chance_percent))}% critical strike (double damage)`;
    case "revive_once": return "revive once at full HP";
    case "melee_cleave": {
      const n = Number(p.count);
      if (n >= 8) return "melee hits all enemies in formation";
      return `melee hits front ${escape(String(p.count))} enemies`;
    }
    case "melee_from_second": return "melee from second formation slot (overrides ranged)";
    case "buff_formation_front": {
      const parts = statParts(p);
      const bonus = parts.length ? parts.join(", ") : "stats";
      return (
        `front ally gets ${bonus} while this unit lives`
      );
    }
    default: return escape(p.kind || "property");
  }
}

function propertyList(properties = []) {
  if (!properties.length) return `<div class="tooltip-empty">no properties</div>`;
  return `<ul class="tooltip-props">${properties.map((p) => `<li>${propertyText(p)}</li>`).join("")}</ul>`;
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
  return `<span class="tooltip-item-icon"><img src="/assets/${escape(item.sprite)}" alt="${escape(item.name)}" />${label ? `<span>${escape(label)}</span>` : ""}</span>`;
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
    <div class="tooltip-hero"><img src="/assets/${escape(cd.sprite)}" alt="${escape(cd.name)}" /></div>
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
    <div class="tooltip-hero"><img src="/assets/${escape(cd.sprite)}" alt="${escape(cd.name)}" /></div>
    ${statGrid(stats.base, stats.total)}
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
    <div class="tooltip-hero"><img src="/assets/${escape(c.sprite)}" alt="${escape(cd?.name || c.def_id)}" /></div>
    ${statGrid(base, total, Math.max(0, c.hp || 0))}
    ${formationFrontAuraTooltipSection(c)}
  ${c.revive_charges ? `<div class="tooltip-section">resurrection</div><div>${escape(String(c.revive_charges))} charge${c.revive_charges === 1 ? "" : "s"} remaining</div>` : ""}
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

function requestLeaderboard(page = state.leaderboardPage) {
  send({ type: "leaderboard", page, per_page: LB_PAGE_SIZE });
}

function renderLeaderboard(msg) {
  state.leaderboardPage = msg.page || 1;
  state.leaderboardPageCount = msg.page_count || 1;
  const startRank = (state.leaderboardPage - 1) * LB_PAGE_SIZE + 1;
  $("#lbList").start = startRank;
  $("#lbList").innerHTML = msg.entries.map(e =>
    `<li>${escape(e.name)} — wins <b>${e.wins}</b> · best streak ${e.streak}</li>`
  ).join("") || "<li>no entries yet</li>";
  $("#lbPageInfo").textContent = `page ${state.leaderboardPage} / ${state.leaderboardPageCount}`;
  $("#lbPrev").disabled = state.leaderboardPage <= 1;
  $("#lbNext").disabled = state.leaderboardPage >= state.leaderboardPageCount;
}

function renderRun() {
  const r = state.run;
  if (!r) return;
  if (document.activeElement !== $("#hudNameInput")) {
    $("#hudNameInput").value = r.name;
  }
  $("#hudMoney").textContent = r.money;
  $("#hudWins").textContent = r.wins;
  $("#hudLosses").textContent = r.losses;
  $("#hudStreak").textContent = r.streak;

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
        <img class="portrait" src="/assets/${cd.sprite}" />
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
      socket.innerHTML = item ? `<img src="/assets/${item.sprite}" alt="${escape(item.name)}" />` : label;
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
  if (!slotAccepts(targetSlot, data.itemSlot)) return;
  e.preventDefault();
  e.stopPropagation();
  e.dataTransfer.dropEffect = "move";
  e.currentTarget.classList.add("drag-over");
}
function onItemSocketDragLeave(e) { e.currentTarget.classList.remove("drag-over"); }
function onItemSocketDrop(e) {
  e.preventDefault();
  e.stopPropagation();
  const data = getDrag(e);
  const target = parseInt(e.currentTarget.dataset.teamIdx, 10);
  const targetSlot = e.currentTarget.dataset.itemSlot;
  e.currentTarget.classList.remove("drag-over");
  if (!data || !slotAccepts(targetSlot, data.itemSlot)) return;
  if (e.currentTarget.classList.contains("filled")) { flash("item socket taken", { variant: "info" }); return; }
  if (data.type === "shop_item") {
    const sid = state.run.shop.items[data.slot];
    const def = itemDef(sid);
    const line = def ? insufficientMoneyLine(def.cost) : null;
    if (line) {
      flash(line, { variant: "error", shakeMoney: true, duration: 3600 });
      return;
    }
    send({ type: "buy_item_to_slot", slot: data.slot, target, target_slot: targetSlot });
  } else if (data.type === "team_item") {
    send({ type: "move_item", from_team: data.team, from_slot: data.slot, to_team: target, to_slot: targetSlot });
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
      <img src="/assets/${cd.sprite}" />
      <div class="name">${cd.name}</div>
      <div class="stats">⚔${cd.might} ⚡${cd.reflexes} ✦${cd.wisdom} ❤${cd.hp}</div>
      <div class="cost">$${cd.cost}</div>
    `;
    attachTooltip(c, () => characterTooltip(cd));
    c.onclick = () => {
      if (cantAfford) {
        flash(insufficientMoneyLine(cd.cost), {
          variant: "error",
          shakeMoney: true,
          duration: 3600,
        });
        return;
      }
      send({ type: "buy_character", slot: i });
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
      <img src="/assets/${it.sprite}" />
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
  setNickname($("#nameInput").value, false);
  send({ type: "new_run", player_id: state.playerId, name: state.nickname });
};
$("#resumeBtn").onclick = () => {
  const id = localStorage.getItem(RUN_ID_KEY);
  if (!id) return flash("no saved run", { variant: "info" });
  send({ type: "resume", run_id: id });
};
$("#lbBtn").onclick = () => requestLeaderboard(1);
$("#lbBack").onclick = () => { if (state.run) renderRun(); else show("start"); };
$("#lbPrev").onclick = () => requestLeaderboard(Math.max(1, state.leaderboardPage - 1));
$("#lbNext").onclick = () => requestLeaderboard(Math.min(state.leaderboardPageCount, state.leaderboardPage + 1));
$("#rerollBtn").onclick = () => {
  const line = insufficientRerollLine();
  if (line) {
    flash(line, { variant: "error", shakeMoney: true, duration: 3600 });
    return;
  }
  send({ type: "reroll" });
};
$("#battleBtn").onclick = () => send({ type: "battle" });
$("#nextRoundBtn").onclick = () => send({ type: "next_round" });
$("#goRestart").onclick = () => { localStorage.removeItem(RUN_ID_KEY); state.run = null; show("start"); };
$("#goLb").onclick = () => requestLeaderboard(1);

wireInventoryDrop();
show("start");
connect();
