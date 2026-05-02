// Canvas renderer for combat playback. Plays through events with simple animation.

const SPRITE_W = 96;
const FLOOR_Y = 240;
const CENTER_GAP = 80;

const imgCache = new Map();
function img(src) {
  if (imgCache.has(src)) return imgCache.get(src);
  const i = new Image(); i.src = `/assets/${src}`;
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
    this.max_hp = c.max_hp;
    this.hp = c.hp;
    this.might = c.might;
    this.x = 0; this.y = FLOOR_Y;
    this.targetX = 0;
    this.dead = false;
    this.flashUntil = 0;
    this.frozenUntil = 0;
    this.shake = 0;
  }
}

function layout(left, right, canvas) {
  const cw = canvas.width;
  const cx = cw / 2;
  // Compute step that keeps the farthest sprite fully on-screen, capped at a max.
  const margin = 8;
  const usable = cx - CENTER_GAP / 2 - SPRITE_W - margin; // per-side usable width
  const slots = Math.max(left.length, right.length, 1);
  const step = Math.min(110, slots > 1 ? usable / (slots - 1) : 110);
  // index 0 is front-most (closest to center).
  left.forEach((s, i) => {
    const x = cx - CENTER_GAP / 2 - SPRITE_W - i * step;
    s.targetX = x;
    if (s.x === 0) s.x = s.targetX;
  });
  right.forEach((s, i) => {
    const x = cx + CENTER_GAP / 2 + i * step;
    s.targetX = x;
    if (s.x === 0) s.x = s.targetX;
  });
}

function drawSprite(ctx, s, t) {
  if (s.dead) return;
  const i = img(s.sprite);
  const w = 96, h = 96;
  let x = s.x;
  let y = s.y - h;
  if (s.shake > 0) { x += (Math.random() - .5) * s.shake; y += (Math.random() - .5) * s.shake; }
  ctx.save();
  if (s.flashUntil > t) ctx.filter = "brightness(2.5) hue-rotate(80deg)";
  if (s.side === 1) { ctx.translate(x + w/2, 0); ctx.scale(-1, 1); ctx.translate(-(x + w/2), 0); }
  if (i.complete && i.naturalWidth) ctx.drawImage(i, x, y, w, h);
  // hat
  if (s.hat) {
    const hi = img(s.hat);
    if (hi.complete && hi.naturalWidth) ctx.drawImage(hi, x + 24, y - 28, 48, 48);
  }
  // left hand (drawn on the character's left = visually right when not flipped)
  if (s.left_hand) {
    const wi = img(s.left_hand);
    if (wi.complete && wi.naturalWidth) ctx.drawImage(wi, x + 50, y + 24, 40, 40);
  }
  // right hand (drawn on the character's right = visually left when not flipped)
  if (s.right_hand) {
    const wi = img(s.right_hand);
    if (wi.complete && wi.naturalWidth) ctx.drawImage(wi, x + 6, y + 24, 40, 40);
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

  // HP bar
  const pct = Math.max(0, s.hp) / s.max_hp;
  ctx.fillStyle = "#000"; ctx.fillRect(x, y - 12, w, 6);
  ctx.fillStyle = pct > 0.5 ? "#4af2ff" : pct > 0.25 ? "#fff36c" : "#ff5cf2";
  ctx.fillRect(x, y - 12, w * pct, 6);
  ctx.fillStyle = "#fff"; ctx.font = "12px monospace"; ctx.textAlign = "center";
  ctx.fillText(`${Math.max(0, s.hp)}`, x + w/2, y - 16);
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

export function playBattle(canvas, battleMsg, charDef, itemDef, onDone) {
  const ctx = canvas.getContext("2d");
  const events = battleMsg.events.slice();
  const spritesById = new Map();
  let leftList = [], rightList = [];
  let projectiles = [];
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
          projectiles.push({
            sprite: ev.projectile,
            x: a.x + 32, y: a.y - 64,
            tx: t.x + 32, ty: t.y - 64,
            flip: a.side === 1,
            target: ev.target,
            hit: ev.hit,
            damage: ev.damage,
            t: 0,
          });
        } else {
          // melee lunge
          const dir = a.side === 0 ? 1 : -1;
          a.x += 24 * dir;
          setTimeout(() => { a.x -= 24 * dir; }, 120);
          if (ev.hit) {
            t.flashUntil = performance.now() + 200;
            t.shake = 6;
            setTimeout(() => { t.shake = 0; }, 200);
          }
          log(ev.hit ? `${a.def_id} hits ${t.def_id} for ${ev.damage}` : `${a.def_id} misses ${t.def_id}`);
        }
        break;
      }
      case "hp": {
        const s = spritesById.get(ev.uid); if (s) s.hp = ev.hp;
        break;
      }
      case "freeze": {
        const s = spritesById.get(ev.target); if (s) s.frozenUntil = performance.now() + 1200;
        log(`${s?.def_id ?? "?"} frozen!`);
        break;
      }
      case "heal": {
        const a = spritesById.get(ev.healer);
        const t = spritesById.get(ev.target);
        if (t) { t.flashUntil = performance.now() + 300; }
        log(`${a?.def_id} heals ${t?.def_id} for ${ev.amount}`);
        break;
      }
      case "death": {
        const s = spritesById.get(ev.uid); if (s) { s.dead = true; }
        log(`${s?.def_id ?? "?"} falls`);
        // re-layout remaining
        leftList = leftList.filter(x => !x.dead);
        rightList = rightList.filter(x => !x.dead);
        layout(leftList, rightList, canvas);
        break;
      }
      case "summon": {
        const s = new Sprite(ev.combatant);
        spritesById.set(s.uid, s);
        if (ev.side === 0) leftList.push(s); else rightList.push(s);
        layout(leftList, rightList, canvas);
        log(`summoned ${s.def_id}`);
        break;
      }
      case "end": {
        done = true;
        break;
      }
    }
  }

  function step(now) {
    const dt = now - last; last = now;
    // advance events at ~250ms cadence
    if (now >= nextEventAt && i < events.length) {
      const ev = events[i++];
      applyEvent(ev);
      nextEventAt = now + (ev.type === "attack" || ev.type === "death" || ev.type === "freeze" ? 380 : 120);
    }

    // projectiles
    projectiles = projectiles.filter(p => {
      p.t += dt / 600;
      if (p.t >= 1) {
        const tgt = spritesById.get(p.target);
        if (tgt && p.hit) { tgt.flashUntil = now + 250; tgt.shake = 8; setTimeout(() => tgt.shake = 0, 220); }
        log(p.hit ? `ranged hits for ${p.damage}` : `ranged misses`);
        return false;
      }
      p.x = p.x + (p.tx - p.x) * 0.05;
      p.y = p.y + (p.ty - p.y) * 0.05;
      return true;
    });

    // smooth move sprites
    [...leftList, ...rightList].forEach(s => { s.x += (s.targetX - s.x) * 0.12; });

    // draw
    ctx.clearRect(0, 0, canvas.width, canvas.height);
    // ground line
    ctx.strokeStyle = "rgba(74,242,255,0.5)"; ctx.lineWidth = 2;
    ctx.beginPath(); ctx.moveTo(0, FLOOR_Y); ctx.lineTo(canvas.width, FLOOR_Y); ctx.stroke();
    // sun
    const grad = ctx.createRadialGradient(canvas.width/2, 60, 10, canvas.width/2, 60, 100);
    grad.addColorStop(0, "rgba(255,243,108,0.8)"); grad.addColorStop(1, "transparent");
    ctx.fillStyle = grad; ctx.fillRect(0, 0, canvas.width, 160);

    leftList.forEach(s => drawSprite(ctx, s, now));
    rightList.forEach(s => drawSprite(ctx, s, now));
    projectiles.forEach(p => drawProjectile(ctx, p));

    if (done && projectiles.length === 0 && i >= events.length) {
      onDone?.();
      return;
    }
    requestAnimationFrame(step);
  }
  requestAnimationFrame(step);
}
