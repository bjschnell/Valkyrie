//! Measures how the system webview copes with the web app (ADR-0020, phase 3:
//! "check WebKitGTK on Linux first"): frames drawn and WebSocket traffic while a
//! session prints a burst of output. Against a development daemon only: it lets
//! itself in with the token such a daemon takes from anyone.
//!
//!     cargo run -p valkyrie-desktop --features window --example probe -- <url> <session> <seconds>

use tauri::{Manager, WebviewUrl, WebviewWindowBuilder};

const PROBE: &str = r##"
(() => {
  try { localStorage.setItem("valk.token", "dev-token"); } catch {}
  window.__VALK_DESKTOP__ = true;
  const stats = { sockets: 0, closes: 0, messages: 0, bytes: 0, frames: 0, worstGapMs: 0, screenChanges: 0 };
  const Real = window.WebSocket;
  window.WebSocket = function (...args) {
    const ws = new Real(...args);
    stats.sockets++;
    ws.addEventListener("message", (e) => { stats.messages++; stats.bytes += e.data.length || 0; });
    ws.addEventListener("close", () => stats.closes++);
    return ws;
  };
  window.WebSocket.prototype = Real.prototype;
  for (const k of ["CONNECTING", "OPEN", "CLOSING", "CLOSED"]) window.WebSocket[k] = Real[k];
  window.addEventListener("load", () => {
    location.hash = "#/s/__SESSION__";
    setTimeout(() => {
      const pre = document.querySelector(".screen pre");
      if (pre) new MutationObserver(() => stats.screenChanges++).observe(pre, { childList: true, subtree: true, characterData: true });
      const before = { ...stats };
      let last = performance.now();
      const start = last;
      const tick = (now) => {
        stats.frames++;
        stats.worstGapMs = Math.max(stats.worstGapMs, now - last);
        last = now;
        if (now - start < __SECONDS__ * 1000) return requestAnimationFrame(tick);
        const secs = (now - start) / 1000;
        document.title = "probe " + JSON.stringify({
          agent: navigator.userAgent.replace(/.*(AppleWebKit\S+).*?((Chrome|Version)\S+).*/, "$1 $2"),
          hasScreen: !!pre,
          grid: document.querySelector(".screen")?.className,
          seconds: +secs.toFixed(1),
          fps: +((stats.frames - before.frames) / secs).toFixed(1),
          worstGapMs: Math.round(stats.worstGapMs),
          wsMessagesPerSec: +((stats.messages - before.messages) / secs).toFixed(1),
          wsKBPerSec: +((stats.bytes - before.bytes) / 1024 / secs).toFixed(1),
          screenChangesPerSec: +((stats.screenChanges - before.screenChanges) / secs).toFixed(1),
          sockets: stats.sockets,
          closes: stats.closes,
          hidden: document.hidden,
        });
      };
      stats.worstGapMs = 0;
      requestAnimationFrame(tick);
    }, 2500);
  });
})();
"##;

fn main() {
    let mut args = std::env::args().skip(1);
    let url = args.next().expect("the valk web address");
    let session = args.next().unwrap_or_else(|| "1".into());
    let seconds = args.next().unwrap_or_else(|| "8".into());
    let script = PROBE
        .replace("__SESSION__", &session)
        .replace("__SECONDS__", &seconds);
    tauri::Builder::default()
        .setup(move |app| {
            WebviewWindowBuilder::new(app, "main", WebviewUrl::External(url.parse()?))
                .title("Valkyrie probe")
                .inner_size(1280.0, 860.0)
                .initialization_script(script)
                .on_document_title_changed(|window, title| {
                    if let Some(result) = title.strip_prefix("probe ") {
                        println!("{result}");
                        window.app_handle().exit(0);
                    }
                })
                .build()?;
            Ok(())
        })
        .run(tauri::generate_context!())
        .expect("the probe failed to start");
}
