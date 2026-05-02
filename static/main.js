import { computePosition, autoUpdate, offset, flip, shift } from "https://cdn.jsdelivr.net/npm/@floating-ui/dom/+esm";
import { playBattle } from "/render.js";

const $ = (sel) => document.querySelector(sel);
const $$ = (sel) => Array.from(document.querySelectorAll(sel));

const state = {
  ws: null,
  defs: { characters: [], items: [] },
  consts: {},
  run: null,
  pendingItem: null, // shop item slot waiting for team target
  lastBattle: null,
  battleAnimating: false,
};

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
      localStorage.setItem("runId", msg.run.id);
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
      $("#lbList").innerHTML = msg.entries.map(e => `<li>${escape(e.name)} — streak <b>${e.streak}</b> · wins ${e.wins}</li>`).join("") || "<li>no entries yet</li>";
      break;
    case "error":
      flash(msg.message);
      break;
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
    case "ranged": return `ranged projectile: ${escape(p.projectile)}`;
    case "healer": return "healer";
    case "freeze_on_hit": return `freeze on hit: ${escape(p.sprite)}`;
    case "summon_on_enemy_death": return `summons ${escape(p.species)} on enemy death`;
    case "summon_on_ally_death": return `summons ${escape(p.species)} on ally death`;
    case "melee_cleave": return `melee hits front ${escape(String(p.count))} enemies`;
    case "stat_bonus": {
      const parts = STAT_KEYS
        .map(({ key, label }) => p[key] ? `${label} ${signed(p[key])}` : null)
        .filter(Boolean);
      return parts.length ? `stat bonus: ${parts.join(", ")}` : "stat bonus";
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
  const nonStatItemProps = items.flatMap(({ item }) => item.properties.filter((p) => p.kind !== "stat_bonus"));
  const combinedProps = [...(cd.properties || []), ...nonStatItemProps];
  return `<div class="tooltip-title">${escape(cd.name)}</div>
    <div class="tooltip-meta">$${cd.cost} · equipped value $${items.reduce((sum, { item }) => sum + item.cost, cd.cost)}</div>
    <div class="tooltip-hero"><img src="/assets/${escape(cd.sprite)}" alt="${escape(cd.name)}" /></div>
    ${statGrid(stats.base, stats.total)}
    <div class="tooltip-section">active properties</div>
    ${propertyList(combinedProps)}
    <div class="tooltip-section">equipped items</div>
    ${itemRows}`;
}

function combatantTooltip(c) {
  const cd = charDef(c.def_id);
  const base = cd ? Object.fromEntries(STAT_KEYS.map(({ key }) => [key, cd[key] || 0])) : {
    might: c.might || 0,
    reflexes: c.reflexes || 0,
    wisdom: c.wisdom || 0,
    hp: c.max_hp || c.hp || 0,
  };
  const total = {
    might: c.might || 0,
    reflexes: c.reflexes || 0,
    wisdom: c.wisdom || 0,
    hp: c.max_hp || c.hp || 0,
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
    <div class="tooltip-section">active properties</div>
    ${propertyList(c.properties || [])}
    <div class="tooltip-section">equipped items</div>
    ${itemRows}`;
}

function flash(text) {
  const el = $("#status"); el.textContent = text; el.style.color = "#ff5cf2";
  setTimeout(() => { el.textContent = ""; }, 2200);
}

function renderRun() {
  const r = state.run;
  if (!r) return;
  $("#hudName").textContent = r.name;
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
function itemSocketId(key) {
  return key === "hat" ? "hat" : key === "left_hand" ? "left_hand" : "right_hand";
}
function slotAccepts(targetSlot, itemSlot) {
  if (itemSlot === "hat") return targetSlot === "hat";
  return targetSlot === "left_hand" || targetSlot === "right_hand";
}
function firstFreeSlot(member, itemSlot) {
  if (!member) return null;
  if (itemSlot === "hat") return member.hat ? null : "hat";
  if (!member.left_hand) return "left_hand";
  if (!member.right_hand) return "right_hand";
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
    send({ type: "buy_item", slot: data.slot, target: to });
  } else if (data.type === "team_item") {
    const targetSlot = firstFreeSlot(state.run.build.team[to], data.itemSlot);
    if (!targetSlot) { flash("no open socket"); return; }
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
  if (e.currentTarget.classList.contains("filled")) { flash("item socket taken"); return; }
  if (data.type === "shop_item") {
    send({ type: "buy_item_to_slot", slot: data.slot, target, target_slot: targetSlot });
  } else if (data.type === "team_item") {
    send({ type: "move_item", from_team: data.team, from_slot: data.slot, to_team: target, to_slot: targetSlot });
  }
}

function renderShop() {
  const sc = $("#shopChars"); sc.innerHTML = "";
  state.run.shop.characters.forEach((id, i) => {
    if (!id) { sc.appendChild(emptyCard()); return; }
    const cd = charDef(id);
    const c = document.createElement("div");
    c.className = "card";
    c.innerHTML = `
      <img src="/assets/${cd.sprite}" />
      <div class="name">${cd.name}</div>
      <div class="stats">⚔${cd.might} ⚡${cd.reflexes} ✦${cd.wisdom} ❤${cd.hp}</div>
      <div class="cost">$${cd.cost}</div>
    `;
    attachTooltip(c, () => characterTooltip(cd));
    c.onclick = () => send({ type: "buy_character", slot: i });
    sc.appendChild(c);
  });
  const si = $("#shopItems"); si.innerHTML = "";
  state.run.shop.items.forEach((id, i) => {
    if (!id) { const e = emptyCard(); e.classList.add("item-card"); si.appendChild(e); return; }
    const it = itemDef(id);
    const c = document.createElement("div");
    c.className = "card item-card" + (state.pendingItem === i ? " equip-mode" : "");
    c.draggable = true;
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
    if (!hasMember) { flash("equip onto a character"); return; }
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
$("#newRunBtn").onclick = () => {
  const name = $("#nameInput").value.trim() || "anon";
  send({ type: "new_run", name });
};
$("#resumeBtn").onclick = () => {
  const id = localStorage.getItem("runId");
  if (!id) return flash("no saved run");
  send({ type: "resume", run_id: id });
};
$("#lbBtn").onclick = () => send({ type: "leaderboard" });
$("#lbBack").onclick = () => { if (state.run) renderRun(); else show("start"); };
$("#rerollBtn").onclick = () => send({ type: "reroll" });
$("#battleBtn").onclick = () => send({ type: "battle" });
$("#nextRoundBtn").onclick = () => send({ type: "next_round" });
$("#goRestart").onclick = () => { localStorage.removeItem("runId"); show("start"); };
$("#goLb").onclick = () => send({ type: "leaderboard" });

wireInventoryDrop();
show("start");
connect();
