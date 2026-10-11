import { useEffect, useRef } from "react";

const GLYPHS = "ｱｲｳｴｵｶｷｸｹｺｻｼｽｾｿﾀﾁﾂﾃﾄﾅﾆﾇﾈﾉﾊﾋﾌﾍﾎﾏﾐﾑﾒﾓﾔﾕﾖﾗﾘﾙﾚﾛﾜﾝ0123456789Z:.=*+<>|";
const CELL = 16;
const FRAME_MS = 60;

const pick = () => GLYPHS[Math.floor(Math.random() * GLYPHS.length)];

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
          ctx.fillStyle = trail;
          ctx.fillText(pick(), i * CELL, (row - 1) * CELL);
          ctx.fillStyle = head;
          ctx.fillText(pick(), i * CELL, row * CELL);
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
