// Draws phone screens from the web app: docs/images/phone.png (the inbox and a
// session's chat), docs/images/phone-work.png (a finished agent's changes and the
// backlog board) and docs/images/phone-flag.png (the supervisor's flag on a PR
// that conflicts with main). The app is the built web/dist, and a stand-in daemon
// answers its WebSocket with made-up sessions, so no pairing or live agents are
// needed:
//
//     cd web && npm run build
//     npm i --no-save playwright && npx playwright install chromium
//     node ../docs/demo/phone.mjs ../docs/images/phone.png
//
// The other two are written beside the first.
//
// Run it from a folder where `playwright` (or `playwright-core`) resolves. Set
// CHROMIUM to use a browser of your own. Needs ImageMagick's `magick`.

import { execFileSync } from "node:child_process";
import { readFileSync, mkdtempSync, rmSync } from "node:fs";
import { createRequire } from "node:module";
import { tmpdir } from "node:os";
import { extname, join, resolve } from "node:path";

const require = createRequire(join(process.cwd(), "/"));
const { chromium } = (() => {
  try {
    return require("playwright");
  } catch {
    return require("playwright-core");
  }
})();

const out = resolve(process.argv[2] ?? "docs/images/phone.png");
const beside = (suffix) => out.replace(/(\.png)?$/, `-${suffix}.png`);
const dist = new URL("../../web/dist/", import.meta.url).pathname;
const ORIGIN = "http://valk.test";
const W = 380;
const H = 780;
const now = Date.now();
const ago = (s) => now - s * 1000;

const status = (agent, state, summary, since, seq) => ({
  agent,
  state,
  ask: state === "needs_input" ? "permission" : null,
  summary,
  since_ms: ago(since),
  seq,
  seen: false,
  hooked: true,
});
const session = (id, name, cwd, st, chat) => ({
  id,
  name,
  command: [st.agent],
  cwd,
  pid: 1000 + id,
  created_unix: Math.floor(ago(3600) / 1000),
  title: null,
  clients: 0,
  exited: null,
  status: st,
  ...(chat ? { chat: `/home/you/.claude/projects/${name}.jsonl` } : {}),
});

const SESSIONS = [
  session(1, "api-auth", "/home/you/repos/api", status("claude", "needs_input", "Bash: cargo test -p auth", 41, 7)),
  session(2, "web-ui", "/home/you/repos/web", status("claude", "working", null, 360, 4), true),
  session(3, "migrations", "/home/you/repos/api", status("codex", "review_ready", "Backfill script added, 3 files changed", 840, 9)),
  session(4, "docs", "/home/you/repos/docs", status("claude", "idle", null, 7200, 2)),
  session(5, "checkout-retry", "/home/you/repos/api", status("claude", "blocked", "PR #142 conflicts with main", 1500, 6)),
];
// The supervisor's flag on session 5's pull request, with the step it offers.
const FLAGS = {
  5: {
    reason: "PR #142 conflicts with main",
    pr: "https://github.com/you/api/pull/142",
    seq: 6,
    step: { action: "merge-base", why: "main moved under it; merging it in is the usual fix" },
  },
};
const QUEUE = SESSIONS.filter((s) => ["needs_input", "review_ready", "blocked"].includes(s.status.state)).map((s) => ({
  session: s.id,
  name: s.name,
  cwd: s.cwd,
  status: s.status,
  ...(FLAGS[s.id] ? { flag: FLAGS[s.id] } : {}),
}));

// The backlog: one card in each of the board's lanes.
const API = "/home/you/repos/api";
const item = (id, title, kind, extra) => ({
  id,
  title,
  body: "",
  kind,
  status: "open",
  project: API,
  by: "human",
  boot: 1,
  created: Math.floor(ago(86400 + id * 600) / 1000),
  updated: Math.floor(ago(3600 - id * 60) / 1000),
  ...extra,
});
const ITEMS = [
  item(11, "Auth tests time out on CI", "bug", { sessions: [1], branch: "fix/11-auth-tests" }),
  item(12, "Dark mode flashes white on load", "bug", { sessions: [2], project: "/home/you/repos/web", priority: "p1" }),
  item(13, "Backfill order totals", "chore", { sessions: [3], branch: "chore/13-backfill" }),
  item(14, "Retry failed card payments", "feature", { sessions: [5], branch: "feat/14-checkout-retry", pr: "https://github.com/you/api/pull/142" }),
  item(15, "Export orders as CSV", "feature", { priority: "p1" }),
  item(16, "Rate-limit the login endpoint", "bug", { status: "proposed", by: "claude" }),
  item(17, "Paginate the admin order list", "feature", { blocked_by: [15] }),
  item(9, "Upgrade to React 19", "chore", { status: "shipped", sessions: [], pr: "https://github.com/you/web/pull/131" }),
];
const LAUNCHES = [
  {
    id: 1,
    project: API,
    item: 15,
    title: "Export orders as CSV",
    agent: "claude",
    why: "nothing blocks it, and api has room for another agent",
    made: Math.floor(ago(120) / 1000),
  },
];

// What session 3 changed, as `/api/review/3` returns it.
const REVIEW = {
  root: API,
  branch: "chore/13-backfill",
  omitted: 0,
  files: [
    {
      path: "scripts/backfill_totals.rs",
      status: "added",
      added: 9,
      removed: 0,
      binary: false,
      cut: false,
      patch: [
        "@@ -0,0 +1,9 @@",
        "+/// Fills in `orders.total_cents` for orders written before it existed.",
        "+pub async fn backfill(db: &Pool) -> Result<u64> {",
        "+    let done = sqlx::query(",
        "+        \"UPDATE orders SET total_cents = sum_lines(id)\",",
        "+    )",
        "+    .execute(db)",
        "+    .await?;",
        "+    Ok(done.rows_affected())",
        "+}",
      ].join("\n"),
    },
    {
      path: "src/orders/model.rs",
      status: "modified",
      added: 2,
      removed: 1,
      binary: false,
      cut: false,
      patch: [
        "@@ -14,7 +14,8 @@ pub struct Order {",
        "     pub id: OrderId,",
        "     pub customer: CustomerId,",
        "-    pub total_cents: i64,",
        "+    /// Null for orders older than the backfill.",
        "+    pub total_cents: Option<i64>,",
        "     pub created_at: DateTime<Utc>,",
        " }",
      ].join("\n"),
    },
    {
      path: "migrations/0042_order_totals.sql",
      status: "added",
      added: 1,
      removed: 0,
      binary: false,
      cut: false,
      patch: "@@ -0,0 +1 @@\n+ALTER TABLE orders ADD COLUMN total_cents BIGINT;",
    },
  ],
};
const PROPOSALS = [
  {
    id: 7,
    project: "/home/you/repos/api",
    title: "Use sqlx, not diesel",
    body: "The handlers are async all the way down, and diesel blocks.",
    kind: "constraint",
    status: "proposed",
    created: Math.floor(ago(240) / 1000),
    updated: Math.floor(ago(240) / 1000),
    provenance: { by: "claude", session: "api-auth", cwd: "/home/you/repos/api" },
  },
];

// Claude's permission prompt, as the inbox reads its answers off the screen.
const PROMPT = [
  "● Running the auth tests",
  "  ⎿  $ cargo test -p auth",
  "─".repeat(78),
  " Bash command",
  "",
  "   cargo test -p auth",
  "   Run the auth crate's tests",
  "",
  " Do you want to proceed?",
  " ❯ 1. Yes",
  "   2. Yes, and don't ask again for cargo test",
  "   3. No, and tell Claude what to do differently (esc)",
  "",
  " Esc to cancel · Tab to amend",
].join("\n");

const tool = (id, name, summary, detail, st = "ok") => ({ k: "tool", id, tool: name, summary, status: st, detail });
const CHAT = [
  { k: "user", id: "u1", text: "The settings page flashes white on load in dark mode. Can you fix it?" },
  {
    k: "say",
    id: "s1",
    text: "The theme class is set in a `useEffect`, so the first paint uses the light default. I'll read the saved theme before React mounts instead.",
  },
  tool("t1", "Read", "src/main.tsx", { file: "src/main.tsx" }),
  tool("t2", "Edit", "index.html", { file: "index.html", diff: "+<script>document.documentElement.dataset.theme = localStorage.theme</script>" }),
  tool("t3", "Bash", "npm test", { command: "npm test", result: "48 passed" }),
  {
    k: "say",
    id: "s2",
    text: "Fixed. The theme is now set by a small inline script in `index.html`, before the first paint, so there's no flash. All **48 tests** pass.",
  },
  { k: "user", id: "u2", text: "Nice. Does the same thing happen on the login page?" },
  {
    k: "say",
    id: "s3",
    text: "It used to, since it shares the layout. The inline script covers it too. I'll add a test that loads `/login` with the dark theme saved.",
  },
  tool("t4", "Write", "login.test.tsx", { file: "src/login.test.tsx", content: "" }),
  tool("t5", "Bash", "npm test -- login", { command: "npm test -- login" }, "running"),
];

const empty = (cols, rows) => ({
  full: true,
  size: { cols, rows },
  rows: [],
  cursor: { x: 0, y: 0, visible: false },
  modes: { app_cursor: false, bracketed_paste: false, alt_screen: false },
  title: null,
});

function daemon(ws) {
  const send = (m) => ws.send(JSON.stringify(m));
  ws.onMessage((raw) => {
    const m = JSON.parse(raw);
    const ok = (reply) => send({ t: "ok", req: m.req, reply });
    switch (m.t) {
      case "hello":
        // The app checks it speaks the daemon's protocol; any number works when both match.
        return ok({ t: "hello", protocol: 99, app_protocol: 99, generation: 1, boot: 1 });
      case "list":
        return ok({ t: "sessions", sessions: SESSIONS });
      case "watch_queue":
        ok({ t: "done" });
        send({ t: "queue", items: QUEUE });
        send({ t: "items", items: ITEMS, orders: [], launches: LAUNCHES });
        return send({ t: "proposals", items: PROPOSALS });
      case "dump":
        return ok({ t: "text", text: m.session === 1 ? PROMPT : "" });
      case "attach":
        ok({ t: "done" });
        return send({ t: "screen", session: m.session, update: empty(80, 24) });
      case "chat":
        if (m.session !== null) send({ t: "chat", session: m.session, items: CHAT, reset: true });
        return;
      default:
        if (m.req !== undefined) ok({ t: "done" });
    }
  });
}

const TYPES = { ".html": "text/html", ".js": "text/javascript", ".css": "text/css", ".svg": "image/svg+xml", ".png": "image/png", ".json": "application/json", ".webmanifest": "application/manifest+json" };

const browser = await chromium.launch(process.env.CHROMIUM ? { executablePath: process.env.CHROMIUM } : {});
const context = await browser.newContext({
  viewport: { width: W, height: H },
  deviceScaleFactor: 2,
  isMobile: true,
  hasTouch: true,
  colorScheme: "dark",
  serviceWorkers: "block",
});
await context.addInitScript(() => localStorage.setItem("valk.token", "demo"));
await context.route(`${ORIGIN}/**`, (route) => {
  const path = new URL(route.request().url()).pathname;
  if (path === "/api/review/3") return route.fulfill({ json: REVIEW });
  if (path.startsWith("/api/")) return route.fulfill({ status: 404, body: "" });
  const file = join(dist, path === "/" ? "index.html" : path);
  try {
    return route.fulfill({ body: readFileSync(file), contentType: TYPES[extname(file)] ?? "application/octet-stream" });
  } catch {
    return route.fulfill({ body: readFileSync(join(dist, "index.html")), contentType: "text/html" });
  }
});
await context.routeWebSocket(`ws://valk.test/ws`, daemon);

const dir = mkdtempSync(join(tmpdir(), "valk-phone-"));
const page = await context.newPage();
await page.goto(`${ORIGIN}/`);
await page.getByText("Use sqlx, not diesel").waitFor();
await page.getByText("Yes, and don't ask again for cargo test").waitFor();
await page.waitForTimeout(1000);
await page.screenshot({ path: join(dir, "inbox.png"), animations: "disabled" });
// The done card above it, just under the app's header, so nothing is cut off.
await page.getByText("Backfill script added, 3 files changed").first().evaluate((el) => {
  const top = el.getBoundingClientRect().top + window.scrollY;
  window.scrollTo(0, top - 150);
});
await page.waitForTimeout(500);
await page.screenshot({ path: join(dir, "flag.png"), animations: "disabled" });
await page.goto(`${ORIGIN}/#/s/2`);
await page.getByText("Does the same thing happen").waitFor();
await page.waitForTimeout(1000);
await page.screenshot({ path: join(dir, "chat.png"), animations: "disabled" });
await page.goto(`${ORIGIN}/#/s/3/review`);
await page.getByText("backfill_totals.rs").waitFor();
await page.waitForTimeout(1000);
await page.screenshot({ path: join(dir, "review.png"), animations: "disabled" });
await page.goto(`${ORIGIN}/#/backlog`);
await page.getByText("Retry failed card payments").waitFor();
await page.waitForTimeout(1000);
await page.screenshot({ path: join(dir, "board.png"), animations: "disabled" });
await browser.close();

// Rounded phone screens with a gap between, on a transparent background.
const round = (name) => {
  const src = join(dir, `${name}.png`);
  const dst = join(dir, `${name}-r.png`);
  const [w, h] = [W * 2, H * 2];
  execFileSync("magick", [
    src,
    "(", "-size", `${w}x${h}`, "xc:none", "-fill", "white", "-draw", `roundrectangle 0,0 ${w - 1},${h - 1} 56,56`, ")",
    "-alpha", "set", "-compose", "DstIn", "-composite", dst,
  ]);
  return dst;
};
const pair = (left, right, dst) => {
  execFileSync("magick", [
    round(left),
    "(", "-size", "80x1", "xc:none", ")",
    round(right),
    "-background", "none", "+append", "-resize", "50%", "-strip",
    // 256 colors is plenty for flat UI, and a third the size.
    "-colors", "256", "-define", "png:compression-level=9", dst,
  ]);
  console.log(`wrote ${dst}`);
};
pair("inbox", "chat", out);
pair("review", "board", beside("work"));
execFileSync("magick", [round("flag"), "-resize", "50%", "-strip", "-colors", "256", "-define", "png:compression-level=9", beside("flag")]);
console.log(`wrote ${beside("flag")}`);
rmSync(dir, { recursive: true });
