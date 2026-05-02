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
};

function show(screenId) {
  $$(".screen").forEach((s) => s.classList.add("hidden"));
  $(`#${screenId}`).classList.remove("hidden");
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
      show("battle");
      $("#leftName").textContent = state.run?.name ?? "you";
      $("#rightName").textContent = msg.opponent_name;
      $("#nextRoundBtn").classList.add("hidden");
      $("#battleLog").innerHTML = "";
      playBattle($("#battleCanvas"), msg, charDef, itemDef, () => {
        const log = (t) => {
          const d = document.createElement("div");
          d.textContent = t; $("#battleLog").appendChild(d); $("#battleLog").scrollTop = 1e9;
        };
        if (msg.winner === 0) log(`✦ you win — +$${state.consts.win_reward}`);
        else if (msg.winner === 1) log(`✗ defeat — +$${state.consts.lose_reward}`);
        else log(`— draw —`);
        $("#nextRoundBtn").classList.remove("hidden");
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
    $("#goWins").textContent = r.wins;
    show("gameover");
    return;
  }
  if (r.phase === "battle") {
    // Wait for client to acknowledge battle screen.
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
    socket.title = label;
    if (itemId) {
      const item = itemDef(itemId);
      socket.draggable = true;
      socket.innerHTML = item ? `<img src="/assets/${item.sprite}" alt="${escape(item.name)}" />` : label;
      socket.addEventListener("dragstart", (e) => {
        e.stopPropagation();
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
      setDrag(e, { type: "shop_item", slot: i, itemSlot: itemSocketId(it.slot) });
      c.classList.add("dragging");
    });
    c.addEventListener("dragend", onDragEnd);
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
