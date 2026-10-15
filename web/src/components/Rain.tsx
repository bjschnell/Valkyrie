import { useEffect, useRef } from "react";

const GLYPHS = "ｱｲｳｴｵｶｷｸｹｺｻｼｽｾｿﾀﾁﾂﾃﾄﾅﾆﾇﾈﾉﾊﾋﾌﾍﾎﾏﾐﾑﾒﾓﾔﾕﾖﾗﾘﾙﾚﾛﾜﾝ0123456789Z:.=*+<>|";
const CELL = 16;
const FRAME_MS = 60;

const pick = () => GLYPHS[Math.floor(Math.random() * GLYPHS.length)];

/**
 * Elder Futhark runes as strokes in a unit cell, x right and y down: drawn rather
 * than typed, since few phones have a font with them.
 */
const RUNES: [number, number][][][] = [
  [[[0.3, 0], [0.3, 1]], [[0.3, 0.35], [0.75, 0.05]], [[0.3, 0.6], [0.75, 0.3]]], // ᚠ
  [[[0.25, 1], [0.25, 0], [0.75, 0.3], [0.75, 1]]], // ᚢ
  [[[0.3, 0], [0.3, 1]], [[0.3, 0.25], [0.7, 0.5], [0.3, 0.75]]], // ᚦ
  [[[0.3, 0], [0.3, 1]], [[0.3, 0], [0.7, 0.25]], [[0.3, 0.3], [0.7, 0.55]]], // ᚨ
  [[[0.3, 1], [0.3, 0], [0.7, 0.25], [0.3, 0.5], [0.7, 1]]], // ᚱ
  [[[0.7, 0], [0.3, 0.5], [0.7, 1]]], // ᚲ
  [[[0.2, 0], [0.8, 1]], [[0.8, 0], [0.2, 1]]], // ᚷ
  [[[0.3, 1], [0.3, 0], [0.7, 0.25], [0.3, 0.5]]], // ᚹ
  [[[0.25, 0], [0.25, 1]], [[0.75, 0], [0.75, 1]], [[0.25, 0.35], [0.75, 0.65]]], // ᚺ
  [[[0.5, 0], [0.5, 1]], [[0.25, 0.35], [0.75, 0.65]]], // ᚾ
  [[[0.5, 0], [0.5, 1]]], // ᛁ
  [[[0.5, 1], [0.5, 0]], [[0.2, 0], [0.5, 0.4], [0.8, 0]]], // ᛉ
  [[[0.7, 0], [0.3, 0.35], [0.7, 0.65], [0.3, 1]]], // ᛊ
  [[[0.5, 0], [0.5, 1]], [[0.2, 0.3], [0.5, 0], [0.8, 0.3]]], // ᛏ
  [[[0.3, 0], [0.3, 1], [0.7, 0.75], [0.3, 0.5], [0.7, 0.25], [0.3, 0]]], // ᛒ
  [[[0.25, 1], [0.25, 0], [0.75, 0.4]], [[0.75, 1], [0.75, 0], [0.25, 0.4]]], // ᛗ
  [[[0.3, 1], [0.3, 0], [0.7, 0.3]]], // ᛚ
  [[[0.15, 1], [0.75, 0.4], [0.5, 0.05], [0.25, 0.4], [0.85, 1]]], // ᛟ
  [[[0.2, 0], [0.2, 1], [0.8, 0], [0.8, 1], [0.2, 0]]], // ᛞ
];

function drawRune(ctx: CanvasRenderingContext2D, x: number, y: number) {
  const rune = RUNES[Math.floor(Math.random() * RUNES.length)];
  const w = CELL * 0.6;
  const h = CELL * 0.8;
  const left = x + (CELL - w) / 2;
  const top = y + (CELL - h) / 2;
  ctx.beginPath();
  for (const line of rune) {
    line.forEach(([u, v], i) => (i ? ctx.lineTo : ctx.moveTo).call(ctx, left + u * w, top + v * h));
  }
  ctx.stroke();
}

/**
 * Digital rain behind the page: one drop per column, each leaving a fading trail.
 * Still under reduced motion, and paused while the tab is hidden.
 */
export function Rain() {
  const ref = useRef<HTMLCanvasElement>(null);

  useEffect(() => {
    const canvas = ref.current;
    const ctx = canvas?.getContext("2d");
    if (!canvas || !ctx) return;
    // The look's colors (see --rain-* in styles.css).
    const css = getComputedStyle(document.documentElement);
    const head = css.getPropertyValue("--rain-head").trim() || "#fff";
    const trail = css.getPropertyValue("--rain-trail").trim() || "#888";
    const runes = css.getPropertyValue("--rain-glyphs").trim() === "runes";
    const glyph = (color: string, x: number, y: number) => {
      if (runes) {
        ctx.strokeStyle = color;
        drawRune(ctx, x, y);
      } else {
        ctx.fillStyle = color;
        ctx.fillText(pick(), x, y);
      }
    };
    const still = window.matchMedia("(prefers-reduced-motion: reduce)").matches;
    let drops: number[] = [];
    let speeds: number[] = [];

    const size = () => {
      const dpr = window.devicePixelRatio || 1;
      canvas.width = Math.floor(innerWidth * dpr);
      canvas.height = Math.floor(innerHeight * dpr);
      ctx.setTransform(dpr, 0, 0, dpr, 0, 0);
      ctx.font = `${CELL - 2}px "JetBrains Mono", ui-monospace, monospace`;
      ctx.textBaseline = "top";
      ctx.lineWidth = 1.5;
      ctx.lineCap = "round";
      ctx.lineJoin = "round";
      const rows = Math.ceil(innerHeight / CELL);
      const cols = Math.ceil(innerWidth / CELL);
      // Spread over the screen and above it, so it's raining from the first frame.
      drops = Array.from({ length: cols }, () => Math.floor(Math.random() * rows * 1.5) - rows / 2);
      speeds = Array.from({ length: cols }, () => 0.4 + Math.random() * 0.8);
      ctx.fillStyle = "#000";
      ctx.fillRect(0, 0, innerWidth, innerHeight);
    };

    const step = () => {
      // Fade what's there, so each drop drags a trail.
      ctx.fillStyle = "rgba(0, 0, 0, 0.09)";
      ctx.fillRect(0, 0, innerWidth, innerHeight);
      const rows = innerHeight / CELL;
      drops.forEach((y, i) => {
        const row = Math.floor(y);
        if (row >= 0) {
          glyph(trail, i * CELL, (row - 1) * CELL);
          glyph(head, i * CELL, row * CELL);
        }
        let next = y + speeds[i];
        if (row > rows && Math.random() > 0.96) next = Math.floor(Math.random() * -20);
        drops[i] = next;
      });
    };

    size();
    if (still) {
      for (let i = 0; i < 80; i++) step();
      return;
    }
    let last = 0;
    let raf = 0;
    const loop = (t: number) => {
      raf = requestAnimationFrame(loop);
      if (document.hidden || t - last < FRAME_MS) return;
      last = t;
      step();
    };
    raf = requestAnimationFrame(loop);
    window.addEventListener("resize", size);
    return () => {
      cancelAnimationFrame(raf);
      window.removeEventListener("resize", size);
    };
  }, []);

  return <canvas ref={ref} className="rain" aria-hidden="true" />;
}
