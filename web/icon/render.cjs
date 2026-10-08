// Renders the icon SVGs to the PNGs the manifest and iOS want: node render.cjs
// (needs Playwright with Chromium; PLAYWRIGHT=<path to the module> if not installed here).
const { chromium } = require(process.env.PLAYWRIGHT || "playwright");
const fs = require("fs");
const out = `${__dirname}/../public/icons`;
const jobs = [
  ["rounded.svg", "icon-192.png", 192],
  ["rounded.svg", "icon-512.png", 512],
  ["mark.svg", "maskable-512.png", 512],
  ["mark.svg", "apple-touch-icon.png", 180],
  ["badge.svg", "badge-96.png", 96],
];
(async () => {
  const browser = await chromium.launch();
  const page = await browser.newPage();
  for (const [src, name, size] of jobs) {
    const svg = fs.readFileSync(`${__dirname}/${src}`, "utf8");
    await page.setViewportSize({ width: size, height: size });
    await page.setContent(
      `<style>html,body{margin:0;background:transparent}svg{display:block;width:${size}px;height:${size}px}</style>${svg}`,
    );
    await page.screenshot({ path: `${out}/${name}`, omitBackground: true });
  }
  await browser.close();
})();
