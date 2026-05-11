// Canvas renderer for combat playback. Plays through events with simple animation.

import { mountBasePath } from "./path-base.js";

const BASE_PATH = mountBasePath(import.meta);

const SPRITE_W = 96;
const FLOOR_Y = 240;
const CENTER_GAP = 80;

const imgCache = new Map();
function img(src) {
  if (imgCache.has(src)) return imgCache.get(src);
  const i = new Image(); i.src = `${BASE_PATH}/assets/${src}`;
  imgCache.set(src, i); return i;
}

class Sprite {
  constructor(c) {
    this.uid = c.uid;
    this.def_id = c.def_id;
    this.side = c.side;
    this.sprite = c.sprite;
    this.hat = c.hat_sprite;
    this.left_hand = c.left_hand_sprite;
    this.right_hand = c.right_hand_sprite;
    this.hand_3 = c.hand_3_sprite;
    this.hand_4 = c.hand_4_sprite;
    this.hat_id = c.hat_id;
    this.left_hand_id = c.left_hand_id;
    this.right_hand_id = c.right_hand_id;
    this.hand_3_id = c.hand_3_id;
    this.hand_4_id = c.hand_4_id;
    this.max_hp = c.max_hp;
    this.formation_hp_bonus = c.formation_hp_bonus || 0;
    this.per_ally_hp_bonus = c.per_ally_hp_bonus || 0;
    this.hp = c.hp;
    this.might = c.might;
    this.reflexes = c.reflexes;
    this.wisdom = c.wisdom;
    this.properties = c.properties || [];
    this.revive_charges = c.revive_charges ?? 0;
    this.revive_at_back_charges = c.revive_at_back_charges ?? 0;
    this.flightUntil = 0;
    this.flightFrom = null;
    this.flightDuration = 0;
    this.flightHeight = 0;
    this.mana = c.mana ?? 0;
    this.max_mana = c.max_mana ?? 0;
    this.applied_front_might = c.applied_front_might ?? 0;
    this.applied_front_reflexes = c.applied_front_reflexes ?? 0;
    this.applied_front_wisdom = c.applied_front_wisdom ?? 0;
    this.applied_per_ally_might = c.applied_per_ally_might ?? 0;
    this.applied_per_ally_reflexes = c.applied_per_ally_reflexes ?? 0;
    this.applied_per_ally_wisdom = c.applied_per_ally_wisdom ?? 0;
    this.applied_enemy_reflex_debuff = c.applied_enemy_reflex_debuff ?? 0;
    this.x = 0; this.y = FLOOR_Y;
    this.targetX = 0;
    this.dead = false;
    this.flashUntil = 0;
    this.frozenUntil = 0;
    this.shake = 0;
    this.bounds = null;
    this.tooltipReference = null;
  }
}

function layout(left, right, canvas) {
  const cw = canvas.width;
  const cx = cw / 2;
  const liveLeft = left.filter(s => !s.dead);
  const liveRight = right.filter(s => !s.dead);
  // Compute step that keeps the farthest sprite fully on-screen, capped at a max.
  const margin = 8;
  const usable = cx - CENTER_GAP / 2 - SPRITE_W - margin; // per-side usable width
  const slots = Math.max(liveLeft.length, liveRight.length, 1);
  const step = Math.min(110, slots > 1 ? usable / (slots - 1) : 110);
  // index 0 is front-most (closest to center).
  liveLeft.forEach((s, i) => {
    const x = cx - CENTER_GAP / 2 - SPRITE_W - i * step;
    s.targetX = x;
    if (s.x === 0) s.x = s.targetX;
  });
  liveRight.forEach((s, i) => {
    const x = cx + CENTER_GAP / 2 + i * step;
    s.targetX = x;
    if (s.x === 0) s.x = s.targetX;
  });
}

function drawSprite(ctx, s, t) {
  if (s.dead) { s.bounds = null; return; }
  const i = img(s.sprite);
  const w = 96, h = 96;
  let x = s.x;
  let y = s.y - h;
  let flightProgress = 0;
  let flying = false;
  if (s.flightUntil > t && s.flightDuration > 0) {
    flying = true;
    flightProgress = clamp(1 - (s.flightUntil - t) / s.flightDuration, 0, 1);
    y -= reviveAtBackArcLift(flightProgress) * s.flightHeight;
  } else if (s.flightUntil) {
    s.flightUntil = 0;
    s.flightFrom = null;
  }
  if (s.shake > 0) { x += (Math.random() - .5) * s.shake; y += (Math.random() - .5) * s.shake; }
  s.bounds = { x, y: y - 28, w, h: h + 34 };
  ctx.save();
  const auraAmt =
    (s.applied_front_might || 0) +
    (s.applied_front_reflexes || 0) +
    (s.applied_front_wisdom || 0) +
    (s.formation_hp_bonus || 0) +
    (s.per_ally_hp_bonus || 0);
  if (auraAmt > 0) {
    const cx = x + w / 2;
    const cy = y + h / 2 - 10;
    const pulse = (Math.sin(t / 170 + s.uid) + 1) / 2;
    const strands = 7;
    ctx.lineCap = "round";
    ctx.lineJoin = "round";
    ctx.shadowColor = "rgba(255, 243, 108, 0.9)";
    ctx.shadowBlur = 18 + pulse * 10;
    for (let n = 0; n < strands; n++) {
      const a = t / 520 + s.uid * 0.37 + n * (Math.PI * 2 / strands);
      const rx = w * (0.36 + 0.04 * Math.sin(t / 240 + n));
      const ry = h * (0.46 + 0.05 * Math.cos(t / 210 + n));
      const sx = cx + Math.cos(a) * rx;
      const sy = cy + Math.sin(a) * ry;
      const ex = cx + Math.cos(a + 0.7) * (rx + 10);
      const ey = cy + Math.sin(a + 0.7) * (ry + 8);
      ctx.strokeStyle = n % 2
        ? `rgba(74, 242, 255, ${0.35 + pulse * 0.25})`
        : `rgba(255, 243, 108, ${0.45 + pulse * 0.3})`;
      ctx.lineWidth = 2 + pulse * 1.5;
      ctx.beginPath();
      ctx.moveTo(sx, sy);
      ctx.quadraticCurveTo(cx + Math.cos(a + 0.35) * (rx + 18), cy + Math.sin(a + 0.35) * (ry + 18), ex, ey);
      ctx.stroke();
    }
    const halo = ctx.createRadialGradient(cx, cy, 8, cx, cy, w * 0.7);
    halo.addColorStop(0, "rgba(255, 243, 108, 0.14)");
    halo.addColorStop(0.45, "rgba(74, 242, 255, 0.08)");
    halo.addColorStop(1, "transparent");
    ctx.fillStyle = halo;
    ctx.beginPath();
    ctx.ellipse(cx, cy, w * 0.7, h * 0.72, 0, 0, Math.PI * 2);
    ctx.fill();
    ctx.shadowBlur = 0;
  }
  if (s.flashUntil > t) ctx.filter = "brightness(2.5) hue-rotate(80deg)";
  if (s.side === 1) { ctx.translate(x + w/2, 0); ctx.scale(-1, 1); ctx.translate(-(x + w/2), 0); }
  if (i.complete && i.naturalWidth) ctx.drawImage(i, x, y, w, h);
  // hat
  if (s.hat) {
    const hi = img(s.hat);
    if (hi.complete && hi.naturalWidth) {
      if (flying) {
        const hx = x + 48;
        const hy = y - 4;
        ctx.save();
        ctx.translate(hx, hy);
        ctx.rotate(flightProgress * Math.PI * 14);
        ctx.drawImage(hi, -28, -28, 56, 56);
        ctx.restore();
      } else {
        ctx.drawImage(hi, x + 24, y - 28, 48, 48);
      }
    }
  }
  // Hands: stacks on character left / right (2nd-row items sit lower on the body).
  const ly2 = y + 4, ly1 = y + 28;
  const lw2 = 36, lw1 = 40;
  if (s.hand_3) {
    const wi = img(s.hand_3);
    if (wi.complete && wi.naturalWidth) ctx.drawImage(wi, x + 51, ly2, lw2, lw2);
  }
  if (s.left_hand) {
    const wi = img(s.left_hand);
    if (wi.complete && wi.naturalWidth) ctx.drawImage(wi, x + 48, ly1, lw1, lw1);
  }
  const ry2 = y + 4, ry1 = y + 28;
  const rw2 = 36, rw1 = 40;
  if (s.hand_4) {
    const wi = img(s.hand_4);
    if (wi.complete && wi.naturalWidth) ctx.drawImage(wi, x + 9, ry2, rw2, rw2);
  }
  if (s.right_hand) {
    const wi = img(s.right_hand);
    if (wi.complete && wi.naturalWidth) ctx.drawImage(wi, x + 8, ry1, rw1, rw1);
  }
  ctx.restore();

  // ice cube overlay
  if (s.frozenUntil > t) {
    const iceI = img("ice.webp");
    if (iceI.complete && iceI.naturalWidth) {
      ctx.globalAlpha = 0.7;
      ctx.drawImage(iceI, x - 6, y - 6, w + 12, h + 12);
      ctx.globalAlpha = 1;
    }
  }

  const maxMana = s.max_mana || 0;
  const mana = Math.max(0, s.mana ?? 0);
  const manaBarLift = maxMana > 0 ? 12 : 0;

  // HP / mana bars — narrower and higher so rows don’t overlap sprites or each other
  const barW = w * 0.62;
  const barX = x + (w - barW) / 2;
  const hpBarH = 5;

  const effMax = Math.max(1, s.max_hp + (s.formation_hp_bonus || 0) + (s.per_ally_hp_bonus || 0));
  const pct = Math.max(0, s.hp) / effMax;
  const hpBarY = y - 32 - manaBarLift;
  const hpLblY = y - 37 - manaBarLift;
  ctx.fillStyle = "#000"; ctx.fillRect(barX, hpBarY, barW, hpBarH);
  ctx.fillStyle = pct > 0.5 ? "#4af2ff" : pct > 0.25 ? "#fff36c" : "#ff5cf2";
  ctx.fillRect(barX, hpBarY, barW * pct, hpBarH);
  ctx.fillStyle = "#fff"; ctx.font = "12px monospace"; ctx.textAlign = "center";
  ctx.fillText(`${Math.max(0, s.hp)}`, barX + barW / 2, hpLblY);

  if (maxMana > 0) {
    const manaPct = mana / Math.max(1, maxMana);
    const manaBarY = y - 18;
    const manaBarH = 4;
    ctx.fillStyle = "#000"; ctx.fillRect(barX, manaBarY, barW, manaBarH);
    ctx.fillStyle = manaPct > 0.35 ? "#9b7dff" : manaPct > 0 ? "#c8b8ff" : "#555";
    ctx.fillRect(barX, manaBarY, barW * manaPct, manaBarH);
    ctx.font = "10px monospace";
    ctx.fillText(`${mana}`, barX + barW / 2, y - 8);
  }
}

function drawProjectile(ctx, p) {
  const i = img(p.sprite);
  if (i.complete && i.naturalWidth) {
    ctx.save();
    if (p.flip) { ctx.translate(p.x + 16, 0); ctx.scale(-1, 1); ctx.translate(-(p.x + 16), 0); }
    ctx.drawImage(i, p.x, p.y, 32, 32);
    ctx.restore();
  } else {
    ctx.fillStyle = "#fff36c"; ctx.beginPath(); ctx.arc(p.x + 16, p.y + 16, 8, 0, Math.PI*2); ctx.fill();
  }
}

const clamp = (value, min, max) => Math.max(min, Math.min(max, value));

/** revive_at_back arc: snaps up (linear over `snap`), then cosine ease back to floor */
function reviveAtBackArcLift(progress, snapPortion = 0.065) {
  const p = clamp(progress, 0, 1);
  const snap = snapPortion;
  if (p <= snap) return p / snap;
  const fall = (p - snap) / Math.max(1e-6, 1 - snap);
  return Math.cos(fall * Math.PI * 0.5);
}

const rand = (min, max) => min + Math.random() * (max - min);

function createBattleAudio() {
  let ctx = null;
  let unavailable = false;
  let disposed = false;
  const timeoutIds = new Set();

  function after(ms, fn) {
    const id = setTimeout(() => {
      timeoutIds.delete(id);
      if (!disposed) fn();
    }, ms);
    timeoutIds.add(id);
  }

  function ensure() {
    if (disposed || unavailable) return null;
    try {
      const AudioCtx = window.AudioContext || window.webkitAudioContext;
      if (!AudioCtx) {
        unavailable = true;
        return null;
      }
      ctx ||= new AudioCtx();
      if (ctx.state === "suspended") ctx.resume().catch(() => {});
      return ctx;
    } catch {
      unavailable = true;
      return null;
    }
  }

  function tone(freq, duration = 0.12, volume = 0.05, type = "sine", slideTo = null) {
    const audio = ensure();
    if (!audio) return;
    const now = audio.currentTime;
    const osc = audio.createOscillator();
    const gain = audio.createGain();
    osc.type = type;
    osc.frequency.setValueAtTime(freq, now);
    if (slideTo) osc.frequency.exponentialRampToValueAtTime(Math.max(20, slideTo), now + duration);
    gain.gain.setValueAtTime(0.0001, now);
    gain.gain.exponentialRampToValueAtTime(volume, now + 0.01);
    gain.gain.exponentialRampToValueAtTime(0.0001, now + duration);
    osc.connect(gain).connect(audio.destination);
    osc.start(now);
    osc.stop(now + duration + 0.02);
  }

  function noise(duration = 0.12, volume = 0.06, filter = "bandpass", freq = 1200) {
    const audio = ensure();
    if (!audio) return;
    const buffer = audio.createBuffer(1, Math.max(1, Math.floor(audio.sampleRate * duration)), audio.sampleRate);
    const data = buffer.getChannelData(0);
    for (let i = 0; i < data.length; i++) {
      const fade = 1 - i / data.length;
      data[i] = (Math.random() * 2 - 1) * fade;
    }
    const src = audio.createBufferSource();
    const gain = audio.createGain();
    const biquad = audio.createBiquadFilter();
    src.buffer = buffer;
    biquad.type = filter;
    biquad.frequency.value = freq;
    gain.gain.value = volume;
    src.connect(biquad).connect(gain).connect(audio.destination);
    src.start();
  }

  return {
    hit(critical = false) {
      noise(critical ? 0.16 : 0.09, critical ? 0.09 : 0.055, "bandpass", critical ? 1600 : 950);
      tone(critical ? 180 : 250, critical ? 0.18 : 0.1, critical ? 0.075 : 0.04, "sawtooth", critical ? 70 : 150);
    },
    miss() { noise(0.12, 0.035, "highpass", 1800); },
    projectile() { tone(620, 0.08, 0.025, "triangle", 980); },
    heal() {
      tone(660, 0.12, 0.04, "sine", 990);
      after(55, () => tone(990, 0.12, 0.035, "sine", 1320));
    },
    freeze() {
      noise(0.18, 0.055, "highpass", 2600);
      tone(1200, 0.2, 0.035, "triangle", 420);
    },
    death() {
      tone(110, 0.24, 0.08, "sawtooth", 45);
      noise(0.2, 0.045, "lowpass", 350);
    },
    revive() {
      tone(330, 0.12, 0.04, "sine", 660);
      after(70, () => tone(880, 0.16, 0.045, "triangle", 1320));
    },
    buff() {
      tone(520, 0.1, 0.035, "triangle", 820);
      after(45, () => tone(1040, 0.12, 0.03, "sine", 1560));
    },
    summon() {
      noise(0.14, 0.04, "bandpass", 700);
      tone(420, 0.14, 0.035, "square", 760);
    },
    end() { tone(220, 0.18, 0.04, "triangle", 440); },
    dispose() {
      if (disposed) return;
      disposed = true;
      for (const tid of timeoutIds) clearTimeout(tid);
      timeoutIds.clear();
      const c = ctx;
      ctx = null;
      if (c && c.state !== "closed") c.close().catch(() => {});
    },
  };
}

function canvasPoint(canvas, ev) {
  const rect = canvas.getBoundingClientRect();
  return {
    x: (ev.clientX - rect.left) * (canvas.width / rect.width),
    y: (ev.clientY - rect.top) * (canvas.height / rect.height),
  };
}

function spriteReference(canvas, s) {
  return {
    getBoundingClientRect() {
      const rect = canvas.getBoundingClientRect();
      const scaleX = rect.width / canvas.width;
      const scaleY = rect.height / canvas.height;
      const b = s.bounds || { x: s.x, y: s.y - 96, w: 96, h: 96 };
      return {
        x: rect.left + b.x * scaleX,
        y: rect.top + b.y * scaleY,
        left: rect.left + b.x * scaleX,
        top: rect.top + b.y * scaleY,
        right: rect.left + (b.x + b.w) * scaleX,
        bottom: rect.top + (b.y + b.h) * scaleY,
        width: b.w * scaleX,
        height: b.h * scaleY,
      };
    },
    contextElement: canvas,
  };
}

function hitSprite(list, point) {
  for (let i = list.length - 1; i >= 0; i--) {
    const s = list[i];
    const b = s.bounds;
    if (!b || s.dead) continue;
    if (point.x >= b.x && point.x <= b.x + b.w && point.y >= b.y && point.y <= b.y + b.h) {
      return s;
    }
  }
  return null;
}

const ATTACK_RESOLUTION_EVENTS = new Set(["hp", "revive", "revive_at_back", "freeze", "death", "death_blast", "stat_sync", "stat_drain", "summon"]);

function isGroupedAttack(ev, group) {
  return ev?.type === "attack" && ev.simultaneous_group != null && ev.simultaneous_group === group;
}

function groupSimultaneousAttacks(rawEvents) {
  const grouped = [];
  for (let idx = 0; idx < rawEvents.length;) {
    const ev = rawEvents[idx];
    if (ev?.type !== "attack" || ev.simultaneous_group == null) {
      grouped.push(ev);
      idx++;
      continue;
    }

    const group = ev.simultaneous_group;
    const attacks = [ev];
    const resolutions = [];
    let cursor = idx + 1;

    while (cursor < rawEvents.length) {
      const chunk = [];
      while (cursor < rawEvents.length && ATTACK_RESOLUTION_EVENTS.has(rawEvents[cursor]?.type)) {
        chunk.push(rawEvents[cursor]);
        cursor++;
      }

      if (isGroupedAttack(rawEvents[cursor], group)) {
        resolutions.push(...chunk);
        attacks.push(rawEvents[cursor]);
        cursor++;
        continue;
      }

      resolutions.push(...chunk);
      break;
    }

    if (attacks.length > 1) {
      grouped.push({ type: "attack_batch", attacks, resolutions });
      idx = cursor;
    } else {
      grouped.push(ev);
      idx++;
    }
  }
  return grouped;
}

export function playBattle(canvas, battleMsg, charDef, itemDef, onDone, tooltip = {}) {
  if (canvas.__battleTooltipCleanup) canvas.__battleTooltipCleanup();
  let rafId = null;
  let cancelled = false;
  const loopReplay = !!tooltip.loop;
  const ctx = canvas.getContext("2d");
  const events = groupSimultaneousAttacks(battleMsg.events);
  const spritesById = new Map();
  let leftList = [], rightList = [];
  let projectiles = [];
  let particles = [];
  let floaters = [];
  let screenShake = 0;
  let audio = createBattleAudio();
  let log = (text) => {
    const el = document.getElementById("battleLog");
    if (!el) return;
    const d = document.createElement("div"); d.textContent = text;
    el.appendChild(d); el.scrollTop = 1e9;
  };

  let i = 0;
  let nextEventAt = 0;
  let last = performance.now();
  let done = false;
  let hovered = null;
  let battleStartedAt = last;

  function playbackSpeed(now = performance.now()) {
    const elapsed = Math.max(0, now - battleStartedAt) / 1000;
    const eventRamp = Math.max(0, i - 1) * 0.018;
    return clamp(1 + elapsed * 0.055 + eventRamp, 1, 2.75);
  }

  function scaledDuration(ms, now = performance.now()) {
    return ms / playbackSpeed(now);
  }

  function eventDelay(ev, now) {
    const heavy = ev.type === "attack" || ev.type === "attack_batch" || ev.type === "death" || ev.type === "death_blast" || ev.type === "freeze" || ev.type === "revive" || ev.type === "revive_at_back";
    return (heavy ? 380 : 120) / playbackSpeed(now);
  }

  function motionRatio(base, dtScaled) {
    return 1 - Math.pow(1 - base, dtScaled / 16.67);
  }

  function spriteAnchor(s) {
    return { x: s.x + 48, y: s.y - 58 };
  }

  function addShake(amount) {
    screenShake = Math.max(screenShake, amount);
  }

  function emitFloater(s, text, opts = {}) {
    if (!s) return;
    const p = spriteAnchor(s);
    floaters.push({
      text,
      x: p.x + rand(-16, 16),
      y: p.y + rand(-10, 8),
      vx: opts.vx ?? rand(-0.18, 0.18),
      vy: opts.vy ?? -0.75,
      color: opts.color ?? "#fff36c",
      stroke: opts.stroke ?? "rgba(0,0,0,0.8)",
      size: opts.size ?? 18,
      ttl: opts.ttl ?? 720,
      life: 0,
      wobble: rand(0, Math.PI * 2),
    });
    if (floaters.length > 48) floaters.splice(0, floaters.length - 48);
  }

  function emitParticles(x, y, count, opts = {}) {
    for (let n = 0; n < count; n++) {
      const angle = opts.angle ?? rand(0, Math.PI * 2);
      const spread = opts.spread ?? Math.PI * 2;
      const a = angle + rand(-spread / 2, spread / 2);
      const speed = rand(opts.minSpeed ?? 0.7, opts.maxSpeed ?? 3.2);
      particles.push({
        x: x + rand(-(opts.jitter ?? 6), opts.jitter ?? 6),
        y: y + rand(-(opts.jitter ?? 6), opts.jitter ?? 6),
        vx: Math.cos(a) * speed,
        vy: Math.sin(a) * speed,
        gravity: opts.gravity ?? 0.035,
        color: opts.altColor && Math.random() > 0.55 ? opts.altColor : opts.color ?? "#fff36c",
        size: rand(opts.minSize ?? 2, opts.maxSize ?? 5),
        ttl: rand(opts.minTtl ?? 260, opts.maxTtl ?? 520),
        life: 0,
        glow: opts.glow ?? 0,
        layer: opts.layer ?? "front",
      });
    }
    if (particles.length > 220) particles.splice(0, particles.length - 220);
  }

  function emitHit(t, ev, x = null, y = null) {
    if (!t) return;
    const p = x == null || y == null ? spriteAnchor(t) : { x, y };
    if (ev.hit) {
      emitFloater(t, `-${ev.damage}`, {
        color: ev.critical ? "#ff5cf2" : "#fff36c",
        size: ev.critical ? 28 : 20,
        ttl: ev.critical ? 860 : 690,
        vy: ev.critical ? -1.1 : -0.82,
      });
      emitParticles(p.x, p.y, ev.critical ? 28 : 16, {
        color: ev.critical ? "#ff5cf2" : "#fff36c",
        altColor: ev.critical ? "#4af2ff" : "#ff8a5c",
        minSpeed: 1.2,
        maxSpeed: ev.critical ? 5.2 : 3.6,
        minSize: 2,
        maxSize: ev.critical ? 7 : 5,
        glow: ev.critical ? 16 : 8,
      });
      addShake(ev.critical ? 10 : 5);
      audio.hit(ev.critical);
    } else {
      emitFloater(t, "MISS", {
        color: "#c8b8ff",
        size: 18,
        ttl: 620,
        vx: t.side === 0 ? -0.42 : 0.42,
        vy: -0.48,
      });
      emitParticles(p.x, p.y, 8, {
        color: "#c8b8ff",
        minSpeed: 1,
        maxSpeed: 2.6,
        gravity: 0,
        minTtl: 180,
        maxTtl: 340,
      });
      audio.miss();
    }
  }

  function emitHeal(t, amount) {
    if (!t) return;
    const p = spriteAnchor(t);
    emitFloater(t, `+${amount}`, { color: "#66ff99", size: 20, ttl: 760, vy: -0.72 });
    emitParticles(p.x, p.y + 10, 20, {
      color: "#66ff99",
      altColor: "#4af2ff",
      minSpeed: 0.5,
      maxSpeed: 2.2,
      gravity: -0.02,
      minSize: 2,
      maxSize: 6,
      glow: 12,
    });
    audio.heal();
  }

  function emitStatus(t, text, color) {
    if (!t) return;
    emitFloater(t, text, { color, size: 18, ttl: 760, vy: -0.62 });
  }

  function emitActivation(t, label = "POWER UP") {
    if (!t) return;
    const p = spriteAnchor(t);
    const now = performance.now();
    t.flashUntil = now + scaledDuration(320, now);
    emitFloater(t, label, {
      color: "#fff36c",
      stroke: "rgba(68, 20, 98, 0.95)",
      size: label.length > 8 ? 16 : 18,
      ttl: 820,
      vy: -0.78,
    });
    emitParticles(p.x, p.y - 2, 26, {
      color: "#fff36c",
      altColor: "#ff5cf2",
      minSpeed: 0.5,
      maxSpeed: 3.4,
      gravity: -0.018,
      minSize: 2,
      maxSize: 7,
      minTtl: 360,
      maxTtl: 720,
      glow: 18,
      layer: "front",
    });
    addShake(3);
    audio.buff();
  }

  const onMouseMove = (ev) => {
    const point = canvasPoint(canvas, ev);
    const hit = hitSprite([...leftList, ...rightList], point);
    if (!hit) {
      if (hovered) tooltip.hideTooltipNow?.();
      hovered = null;
      return;
    }
    hovered = hit;
    hit.tooltipReference ||= spriteReference(canvas, hit);
    tooltip.showTooltip?.(hit.tooltipReference, hit);
  };
  const onMouseLeave = () => {
    hovered = null;
    tooltip.hideTooltipNow?.();
  };
  canvas.addEventListener("mousemove", onMouseMove);
  canvas.addEventListener("mouseleave", onMouseLeave);
  canvas.__battleTooltipCleanup = () => {
    cancelled = true;
    if (rafId != null) {
      cancelAnimationFrame(rafId);
      rafId = null;
    }
    audio.dispose();
    canvas.removeEventListener("mousemove", onMouseMove);
    canvas.removeEventListener("mouseleave", onMouseLeave);
    tooltip.hideTooltipNow?.();
  };

  function applyEvent(ev) {
    switch (ev.type) {
      case "start":
        leftList = ev.left.map(c => new Sprite(c));
        rightList = ev.right.map(c => new Sprite(c));
        leftList.forEach(s => spritesById.set(s.uid, s));
        rightList.forEach(s => spritesById.set(s.uid, s));
        layout(leftList, rightList, canvas);
        break;
      case "attack": {
        const a = spritesById.get(ev.attacker);
        const t = spritesById.get(ev.target);
        if (!a || !t) break;
        if (ev.ranged && ev.projectile) {
          const aimX = t.x + 32;
          const aimY = t.y - 64;
          // Same hit/miss as melee (server: reflex vs reflex). Miss flies past/side so it reads as dodged.
          const towardEnemy = a.side === 0 ? 1 : -1;
          let tx = aimX;
          let ty = aimY;
          if (!ev.hit) {
            tx = aimX + towardEnemy * (48 + Math.random() * 36);
            ty = aimY + (Math.random() - 0.5) * 56;
          }
          projectiles.push({
            sprite: ev.projectile,
            x: a.x + 32, y: a.y - 64,
            tx, ty,
            flip: a.side === 1,
            target: ev.target,
            hit: ev.hit,
            damage: ev.damage,
            critical: ev.critical,
            t: 0,
          });
          audio.projectile();
          log(ev.hit ? `${a.def_id} hits ${t.def_id} for ${ev.damage}${ev.critical ? " (crit!)" : ""}` : `${a.def_id} misses ${t.def_id}`);
        } else {
          // melee lunge
          const dir = a.side === 0 ? 1 : -1;
          a.x += 24 * dir;
          setTimeout(() => { a.x -= 24 * dir; }, scaledDuration(120));
          if (ev.hit) {
            const now = performance.now();
            t.flashUntil = now + scaledDuration(200, now);
            t.shake = 6;
            setTimeout(() => { t.shake = 0; }, scaledDuration(200, now));
          }
          emitHit(t, ev);
          log(ev.hit ? `${a.def_id} hits ${t.def_id} for ${ev.damage}${ev.critical ? " (crit!)" : ""}` : `${a.def_id} misses ${t.def_id}`);
        }
        break;
      }
      case "attack_batch": {
        const first = ev.attacks[0];
        const a = spritesById.get(first?.attacker);
        if (!a) break;
        const dir = a.side === 0 ? 1 : -1;
        a.x += 24 * dir;
        setTimeout(() => { a.x -= 24 * dir; }, scaledDuration(120));

        ev.attacks.forEach(atk => {
          const t = spritesById.get(atk.target);
          if (!t) return;
          if (atk.hit) {
            const now = performance.now();
            t.flashUntil = now + scaledDuration(200, now);
            t.shake = 6;
            setTimeout(() => { t.shake = 0; }, scaledDuration(200, now));
          }
          emitHit(t, atk);
          log(atk.hit ? `${a.def_id} hits ${t.def_id} for ${atk.damage}${atk.critical ? " (crit!)" : ""}` : `${a.def_id} misses ${t.def_id}`);
        });
        ev.resolutions.forEach(applyEvent);
        break;
      }
      case "stat_drain": {
        const s = spritesById.get(ev.uid);
        if (s) {
          const amt = Math.max(0, Number(ev.amount) || 0);
          if (amt > 0) {
            const now = performance.now();
            s.flashUntil = Math.max(s.flashUntil || 0, now + scaledDuration(160, now));
            const p = spriteAnchor(s);
            emitFloater(s, `-${amt} ALL STATS`, {
              color: "#7ec8e3",
              stroke: "rgba(20, 40, 55, 0.92)",
              size: amt > 9 ? 15 : 17,
              ttl: 820,
              vy: -0.58,
            });
            emitParticles(p.x, p.y + 4, 14, {
              color: "#7ec8e3",
              altColor: "#4af2ff",
              minSpeed: 0.5,
              maxSpeed: 2.4,
              gravity: 0.02,
              minSize: 2,
              maxSize: 5,
              glow: 10,
            });
            addShake(2);
          }
        }
        break;
      }
      case "stat_sync": {
        const s = spritesById.get(ev.uid);
        if (s) {
          const deltaMight = ev.might - s.might;
          const deltaReflexes = ev.reflexes - s.reflexes;
          const deltaWisdom = ev.wisdom - s.wisdom;
          const deltaMaxHp = ev.max_hp - s.max_hp;
          const positiveStats = [deltaMight, deltaReflexes, deltaWisdom, deltaMaxHp].filter(n => n > 0);
          const showActivation =
            positiveStats.length > 0 &&
            ev.applied_front_might === s.applied_front_might &&
            ev.applied_front_reflexes === s.applied_front_reflexes &&
            ev.applied_front_wisdom === s.applied_front_wisdom &&
            (ev.formation_hp_bonus || 0) === (s.formation_hp_bonus || 0) &&
            (ev.per_ally_hp_bonus || 0) === (s.per_ally_hp_bonus || 0);
          s.might = ev.might;
          s.reflexes = ev.reflexes;
          s.wisdom = ev.wisdom;
          s.max_hp = ev.max_hp;
          s.hp = ev.hp;
          s.formation_hp_bonus = ev.formation_hp_bonus || 0;
          s.per_ally_hp_bonus = ev.per_ally_hp_bonus || 0;
          s.applied_front_might = ev.applied_front_might ?? 0;
          s.applied_front_reflexes = ev.applied_front_reflexes ?? 0;
          s.applied_front_wisdom = ev.applied_front_wisdom ?? 0;
          s.applied_per_ally_might = ev.applied_per_ally_might ?? 0;
          s.applied_per_ally_reflexes = ev.applied_per_ally_reflexes ?? 0;
          s.applied_per_ally_wisdom = ev.applied_per_ally_wisdom ?? 0;
          s.applied_enemy_reflex_debuff = ev.applied_enemy_reflex_debuff ?? 0;
          if (showActivation) {
            const label = positiveStats.every(n => n === positiveStats[0]) && positiveStats.length >= 3
              ? `ALL +${positiveStats[0]}`
              : "POWER UP";
            emitActivation(s, label);
          }
        }
        break;
      }
      case "hp": {
        const s = spritesById.get(ev.uid); if (s) s.hp = ev.hp;
        break;
      }
      case "mana": {
        const s = spritesById.get(ev.uid);
        if (s) s.mana = ev.mana;
        break;
      }
      case "freeze": {
        const s = spritesById.get(ev.target);
        if (s) {
          const now = performance.now();
          const p = spriteAnchor(s);
          s.frozenUntil = now + scaledDuration(1200, now);
          emitStatus(s, "FROZEN", "#8eeaff");
          emitParticles(p.x, p.y, 30, {
            color: "#8eeaff",
            altColor: "#ffffff",
            minSpeed: 1.1,
            maxSpeed: 4.4,
            gravity: 0.006,
            minSize: 2,
            maxSize: 6,
            glow: 14,
          });
          addShake(6);
          audio.freeze();
        }
        log(`${s?.def_id ?? "?"} frozen!`);
        break;
      }
      case "heal": {
        const a = spritesById.get(ev.healer);
        const t = spritesById.get(ev.target);
        if (t) {
          const now = performance.now();
          t.flashUntil = now + scaledDuration(300, now);
          emitHeal(t, ev.amount);
        }
        log(`${a?.def_id} heals ${t?.def_id} for ${ev.amount}`);
        break;
      }
      case "death": {
        const s = spritesById.get(ev.uid);
        if (s) {
          const p = spriteAnchor(s);
          emitStatus(s, "KO", "#ff8a5c");
          emitParticles(p.x, p.y + 12, 36, {
            color: "#ff8a5c",
            altColor: "#ff5cf2",
            minSpeed: 0.8,
            maxSpeed: 4.8,
            gravity: 0.05,
            minSize: 3,
            maxSize: 8,
            glow: 10,
          });
          addShake(8);
          audio.death();
          s.dead = true;
        }
        log(`${s?.def_id ?? "?"} falls`);
        layout(leftList, rightList, canvas);
        break;
      }
      case "death_blast": {
        const source = spritesById.get(ev.source);
        const target = spritesById.get(ev.target);
        if (target) {
          const now = performance.now();
          target.flashUntil = now + scaledDuration(250, now);
          target.shake = 9;
          setTimeout(() => { target.shake = 0; }, scaledDuration(220, now));
          emitStatus(target, "DEATH BLAST", "#ff5c5c");
          emitHit(target, { hit: true, damage: ev.damage, critical: false });
        }
        log(`${source?.def_id ?? "death blast"} hits ${target?.def_id ?? "?"} for ${ev.damage}`);
        break;
      }
      case "revive": {
        const s = spritesById.get(ev.uid);
        if (s) {
          s.dead = false;
          s.hp = ev.hp;
          s.revive_charges = Math.max(0, (s.revive_charges || 0) - 1);
          const now = performance.now();
          const p = spriteAnchor(s);
          s.flashUntil = now + scaledDuration(350, now);
          emitStatus(s, "REVIVE", "#66ff99");
          emitParticles(p.x, p.y, 32, {
            color: "#66ff99",
            altColor: "#fff36c",
            minSpeed: 0.6,
            maxSpeed: 3.2,
            gravity: -0.025,
            minSize: 2,
            maxSize: 7,
            glow: 18,
          });
          addShake(4);
          audio.revive();
          layout(leftList, rightList, canvas);
        }
        log(`${s?.def_id ?? "?"} resurrects!`);
        break;
      }
      case "revive_at_back": {
        const s = spritesById.get(ev.uid);
        if (s) {
          const list = s.side === 0 ? leftList : rightList;
          // Move to back of formation order so layout slots them last.
          const idx = list.indexOf(s);
          if (idx >= 0) {
            list.splice(idx, 1);
            list.push(s);
          }
          s.dead = false;
          s.hp = ev.hp;
          s.revive_at_back_charges = Math.max(0, (s.revive_at_back_charges || 0) - 1);
          const now = performance.now();
          const fromX = s.x;
          const fromY = s.y;
          layout(leftList, rightList, canvas);
          // Layout updated targetX; keep current x so the tween + flight arc carries it back.
          s.x = fromX;
          s.flightFrom = { x: fromX, y: fromY };
          s.flightDuration = scaledDuration(1350, now);
          s.flightUntil = now + s.flightDuration;
          s.flightHeight = 140;
          s.flashUntil = now + scaledDuration(350, now);
          emitStatus(s, "PROPELLOR!", "#fff36c");
          const p = spriteAnchor(s);
          emitParticles(p.x, p.y, 28, {
            color: "#fff36c",
            altColor: "#4af2ff",
            minSpeed: 0.6,
            maxSpeed: 3.4,
            gravity: -0.02,
            minSize: 2,
            maxSize: 7,
            glow: 18,
          });
          addShake(5);
          audio.revive();
        }
        log(`${s?.def_id ?? "?"} zips to the back!`);
        break;
      }
      case "summon": {
        const s = new Sprite(ev.combatant);
        spritesById.set(s.uid, s);
        const list = ev.side === 0 ? leftList : rightList;
        const summoner = spritesById.get(ev.summoner);
        if (summoner && list.includes(summoner)) {
          list.splice(list.indexOf(summoner), 0, s);
        } else {
          list.push(s);
        }
        layout(leftList, rightList, canvas);
        emitStatus(s, "SUMMON", "#c8b8ff");
        emitParticles(s.x + 48, s.y - 48, 28, {
          color: "#c8b8ff",
          altColor: "#4af2ff",
          minSpeed: 0.6,
          maxSpeed: 3,
          gravity: -0.01,
          minSize: 3,
          maxSize: 8,
          glow: 12,
          layer: "back",
        });
        audio.summon();
        log(`summoned ${s.def_id}`);
        break;
      }
      case "lock_breaker": {
        const result = ev.winner === 0 ? "you win" : ev.winner === 1 ? "enemy wins" : "draw";
        log(`lock breaker: ${result} by living gold ${ev.left_living_gold}-${ev.right_living_gold}`);
        break;
      }
      case "end": {
        done = true;
        audio.end();
        break;
      }
    }
  }

  function updateParticles(dtScaled) {
    particles = particles.filter(p => {
      const frame = dtScaled / 16.67;
      p.life += dtScaled;
      p.x += p.vx * frame;
      p.y += p.vy * frame;
      p.vy += p.gravity * frame;
      p.size *= Math.pow(0.985, frame);
      return p.life < p.ttl && p.size > 0.3;
    });
  }

  function drawParticles(ctx, layer) {
    particles.forEach(p => {
      if (p.layer !== layer) return;
      const alpha = clamp(1 - p.life / p.ttl, 0, 1);
      ctx.save();
      ctx.globalAlpha = alpha;
      ctx.fillStyle = p.color;
      if (p.glow) {
        ctx.shadowColor = p.color;
        ctx.shadowBlur = p.glow * alpha;
      }
      ctx.beginPath();
      ctx.arc(p.x, p.y, p.size, 0, Math.PI * 2);
      ctx.fill();
      ctx.restore();
    });
  }

  function updateFloaters(dtScaled) {
    floaters = floaters.filter(f => {
      const frame = dtScaled / 16.67;
      f.life += dtScaled;
      f.x += f.vx * frame + Math.sin(f.life / 90 + f.wobble) * 0.12 * frame;
      f.y += f.vy * frame;
      f.vy += 0.006 * frame;
      return f.life < f.ttl;
    });
  }

  function drawFloaters(ctx) {
    floaters.forEach(f => {
      const progress = clamp(f.life / f.ttl, 0, 1);
      const alpha = Math.sin((1 - progress) * Math.PI * 0.5);
      const pop = 1 + Math.max(0, 0.28 - progress) * 1.1;
      ctx.save();
      ctx.globalAlpha = alpha;
      ctx.translate(f.x, f.y);
      ctx.scale(pop, pop);
      ctx.font = `900 ${f.size}px monospace`;
      ctx.textAlign = "center";
      ctx.lineWidth = 4;
      ctx.strokeStyle = f.stroke;
      ctx.fillStyle = f.color;
      ctx.shadowColor = f.color;
      ctx.shadowBlur = 10 * alpha;
      ctx.strokeText(f.text, 0, 0);
      ctx.fillText(f.text, 0, 0);
      ctx.restore();
    });
  }

  function step(now) {
    if (cancelled) return;
    const dt = now - last; last = now;
    const speed = playbackSpeed(now);
    const dtScaled = dt * speed;
    // advance events at ~250ms cadence
    if (now >= nextEventAt && i < events.length) {
      const ev = events[i++];
      applyEvent(ev);
      nextEventAt = now + eventDelay(ev, now);
    }

    // projectiles
    projectiles = projectiles.filter(p => {
      p.t += dtScaled / 600;
      if (p.t >= 1) {
        const tgt = spritesById.get(p.target);
        if (tgt) {
          if (p.hit) {
            tgt.flashUntil = now + scaledDuration(250, now);
            tgt.shake = 8;
            setTimeout(() => tgt.shake = 0, scaledDuration(220, now));
          }
          emitHit(tgt, p, p.tx + 16, p.ty + 16);
        }
        return false;
      }
      const ratio = motionRatio(0.05, dtScaled);
      p.x = p.x + (p.tx - p.x) * ratio;
      p.y = p.y + (p.ty - p.y) * ratio;
      return true;
    });

    // smooth move sprites
    const moveRatio = motionRatio(0.12, dtScaled);
    const flightMoveRatio = motionRatio(0.055, dtScaled);
    [...leftList, ...rightList].forEach(s => {
      const flying = s.flightUntil > now && s.flightDuration > 0;
      s.x += (s.targetX - s.x) * (flying ? flightMoveRatio : moveRatio);
      if (flying) {
        const progress = clamp(1 - (s.flightUntil - now) / s.flightDuration, 0, 1);
        const trailY = s.y - 96 - reviveAtBackArcLift(progress) * (s.flightHeight || 0) + 24;
        emitParticles(s.x + 48, trailY, 2, {
          color: "#fff36c",
          altColor: "#4af2ff",
          minSpeed: 0.4,
          maxSpeed: 1.6,
          gravity: 0.02,
          minSize: 2,
          maxSize: 4,
          glow: 10,
          layer: "back",
          minTtl: 220,
          maxTtl: 420,
        });
      }
    });
    updateParticles(dtScaled);
    updateFloaters(dtScaled);
    screenShake = Math.max(0, screenShake - dtScaled * 0.045);

    // draw
    ctx.clearRect(0, 0, canvas.width, canvas.height);
    ctx.save();
    if (screenShake > 0) ctx.translate(rand(-screenShake, screenShake), rand(-screenShake, screenShake));
    // ground line
    ctx.strokeStyle = "rgba(74,242,255,0.5)"; ctx.lineWidth = 2;
    ctx.beginPath(); ctx.moveTo(0, FLOOR_Y); ctx.lineTo(canvas.width, FLOOR_Y); ctx.stroke();
    // sun
    const grad = ctx.createRadialGradient(canvas.width/2, 60, 10, canvas.width/2, 60, 100);
    grad.addColorStop(0, "rgba(255,243,108,0.8)"); grad.addColorStop(1, "transparent");
    ctx.fillStyle = grad; ctx.fillRect(0, 0, canvas.width, 160);

    drawParticles(ctx, "back");
    leftList.forEach(s => drawSprite(ctx, s, now));
    rightList.forEach(s => drawSprite(ctx, s, now));
    projectiles.forEach(p => drawProjectile(ctx, p));
    drawParticles(ctx, "front");
    drawFloaters(ctx);
    ctx.restore();
    if (hovered?.dead) {
      tooltip.hideTooltipNow?.();
      hovered = null;
    }
    if (hovered && !hovered.dead) {
      hovered.tooltipReference ||= spriteReference(canvas, hovered);
      tooltip.showTooltip?.(hovered.tooltipReference, hovered);
    }

    if (done && projectiles.length === 0 && floaters.length === 0 && particles.length === 0 && i >= events.length) {
      if (loopReplay) {
        i = 0;
        const nowReset = performance.now();
        nextEventAt = nowReset;
        last = nowReset;
        done = false;
        battleStartedAt = nowReset;
        spritesById.clear();
        leftList = [];
        rightList = [];
        projectiles = [];
        particles = [];
        floaters = [];
        screenShake = 0;
        hovered = null;
        tooltip.hideTooltipNow?.();
        audio.dispose();
        audio = createBattleAudio();
      } else {
        onDone?.();
        rafId = null;
        return;
      }
    }
    if (!cancelled) rafId = requestAnimationFrame(step);
  }
  rafId = requestAnimationFrame(step);
}
