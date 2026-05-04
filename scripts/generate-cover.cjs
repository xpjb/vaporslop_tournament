/**
 * Builds static/cover-630x500.png — OG-style cover matching #bg texture,
 * orang vs meme man, and header-style pink wash + title stack.
 */
const { mkdirSync, writeFileSync } = require("fs");
const { join } = require("path");
const { createCanvas, loadImage, GlobalFonts } = require("@napi-rs/canvas");

const W = 630;
const H = 500;

const ROOT = join(__dirname, "..");
const ASSETS = join(ROOT, "assets");
const OUT = join(ROOT, "static", "cover-630x500.png");
const FONT = join(
  ROOT,
  "node_modules",
  "@fontsource",
  "vt323",
  "files",
  "vt323-latin-400-normal.woff"
);

function drawMainBackground(ctx) {
  ctx.fillStyle = "#0d0028";
  ctx.fillRect(0, 0, W, H);

  const prev = ctx.globalCompositeOperation;
  ctx.globalCompositeOperation = "screen";
  const rg = ctx.createRadialGradient(W / 2, 0, 0, W / 2, H * 0.35, H * 0.95);
  rg.addColorStop(0, "#2a0a4f");
  rg.addColorStop(0.55, "#0d0028");
  rg.addColorStop(1, "#02000c");
  ctx.fillStyle = rg;
  ctx.fillRect(0, 0, W, H);

  for (let y = 0; y < H; y += 4) {
    ctx.fillStyle = "rgba(255, 92, 242, 0.05)";
    ctx.fillRect(0, y, W, 2);
  }
  ctx.globalCompositeOperation = prev;

  const g2 = ctx.createLinearGradient(0, 0, 0, H);
  g2.addColorStop(0, "rgba(74, 242, 255, 0)");
  g2.addColorStop(0.6, "rgba(74, 242, 255, 0)");
  g2.addColorStop(1, "rgba(74, 242, 255, 0.10)");
  ctx.fillStyle = g2;
  ctx.fillRect(0, 0, W, H);

  ctx.fillStyle = "rgba(255, 92, 242, 0.04)";
  for (let x = 0; x < W; x += 40) {
    ctx.fillRect(x, 0, 1, H);
  }
}

function drawSpriteScaled(ctx, img, cx, baselineY, targetH, flipH) {
  const scale = targetH / img.height;
  const dw = img.width * scale;
  const dh = img.height * scale;
  const x = cx - dw / 2;
  const y = baselineY - dh;
  ctx.save();
  if (flipH) {
    ctx.translate(cx, 0);
    ctx.scale(-1, 1);
    ctx.translate(-cx, 0);
  }
  ctx.drawImage(img, x, y, dw, dh);
  ctx.restore();
}

function drawHeaderWash(ctx) {
  const fadeEnd = H * 0.75;
  const v = ctx.createLinearGradient(0, 0, 0, fadeEnd);
  v.addColorStop(0, "rgba(255, 92, 242, 0.34)");
  v.addColorStop(0.35, "rgba(255, 92, 242, 0.14)");
  v.addColorStop(1, "rgba(255, 92, 242, 0)");
  ctx.fillStyle = v;
  ctx.fillRect(0, 0, W, fadeEnd);

  const h = ctx.createLinearGradient(0, 0, W * 0.85, 0);
  h.addColorStop(0, "rgba(255, 92, 242, 0.22)");
  h.addColorStop(1, "rgba(255, 92, 242, 0)");
  ctx.globalCompositeOperation = "lighter";
  ctx.fillStyle = h;
  ctx.fillRect(0, 0, W, fadeEnd * 0.55);
  ctx.globalCompositeOperation = "source-over";
}

function drawTitle(ctx) {
  const padX = 32;
  const padY = 28;
  let y = padY;

  GlobalFonts.registerFromPath(FONT, "VT323");
  ctx.textBaseline = "top";
  ctx.font = "44px VT323";
  ctx.letterSpacing = "0.14em";

  const title = "VAPORSLOP";
  ctx.shadowColor = "rgba(164, 92, 255, 0.75)";
  ctx.shadowBlur = 22;
  ctx.fillStyle = "#ffffff";
  ctx.fillText(title, padX, y);
  ctx.shadowColor = "rgba(255, 92, 242, 0.9)";
  ctx.shadowBlur = 12;
  ctx.fillText(title, padX, y);
  ctx.shadowBlur = 0;

  y += 46;
  ctx.font = "28px VT323";
  ctx.letterSpacing = "0.22em";
  ctx.fillStyle = "rgba(74, 242, 255, 0.72)";
  ctx.fillText("// T O U R N A M E N T", padX, y);

  const borderY = y + 36;
  ctx.strokeStyle = "#ff5cf2";
  ctx.lineWidth = 2;
  ctx.beginPath();
  ctx.moveTo(0, borderY);
  ctx.lineTo(W, borderY);
  ctx.stroke();
}

async function main() {
  const [orang, meme] = await Promise.all([
    loadImage(join(ASSETS, "orang.webp")),
    loadImage(join(ASSETS, "Meme_Man.webp")),
  ]);

  const canvas = createCanvas(W, H);
  const ctx = canvas.getContext("2d");

  drawMainBackground(ctx);

  const spriteH = 220;
  const baseY = H - 28;
  drawSpriteScaled(ctx, orang, W * 0.28, baseY, spriteH, false);
  drawSpriteScaled(ctx, meme, W * 0.72, baseY, spriteH, true);

  drawHeaderWash(ctx);
  drawTitle(ctx);

  const buf = await canvas.encode("png");
  mkdirSync(join(ROOT, "static"), { recursive: true });
  writeFileSync(OUT, buf);
  console.log("Wrote", OUT);
}

main().catch((e) => {
  console.error(e);
  process.exit(1);
});
