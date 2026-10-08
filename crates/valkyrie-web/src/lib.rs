//! `valk web`: the web app's server (DESIGN §8.5). It is a separate, opt-in process.
//! It serves the app and bridges each browser WebSocket to the daemon's unix socket,
//! so the daemon itself never listens on a network. It binds to localhost; `tailscale
//! serve` puts HTTPS in front of it, reachable only on the tailnet.

pub mod auth;

use anyhow::{Context, Result};
use axum::Router;
use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
use axum::extract::{Json, State};
use axum::http::{HeaderMap, HeaderValue, StatusCode, Uri, header};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use include_dir::{Dir, include_dir};
use serde::Deserialize;
use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;
use tokio::net::UnixStream;
use valkyrie_proto::ClientMsg;
use valkyrie_proto::codec::{read_frame, write_frame};

/// The built app (`web/dist`, committed, so building `valk` needs no Node).
static DIST: Dir = include_dir!("$CARGO_MANIFEST_DIR/../../web/dist");

/// The WebSocket subprotocol; the device token rides as a second one,
/// `bearer.<token>`, since browsers can't set headers on a WebSocket.
const SUBPROTOCOL: &str = "valk.v1";
/// The close code for a device that isn't paired (or was unpaired).
const CLOSE_UNPAIRED: u16 = 4401;
/// Keeps phone connections from being dropped as idle by proxies and radios.
const PING_EVERY: Duration = Duration::from_secs(25);

pub struct Options {
    pub listen: SocketAddr,
    pub socket: PathBuf,
    /// Where phones reach the server, for the pairing QR code. Default: this
    /// machine's tailnet name, else the listen address.
    pub url: Option<String>,
}

struct App {
    store: auth::Store,
    socket: PathBuf,
}

pub fn store() -> Result<auth::Store> {
    auth::Store::open(&valkyrie_proto::state_dir().join("web"))
}

pub async fn run(opts: Options) -> Result<()> {
    let store = store()?;
    let url = public_url(opts.url, opts.listen);
    let listener = tokio::net::TcpListener::bind(opts.listen)
        .await
        .with_context(|| format!("listen on {}", opts.listen))?;
    println!("valk web on http://{}", opts.listen);
    if !opts.listen.ip().is_loopback() {
        println!(
            "  listening beyond localhost: anyone who can reach {} can try to pair",
            opts.listen
        );
    } else if url.starts_with("https://") {
        println!(
            "  reach it from your tailnet with: tailscale serve --bg {}",
            opts.listen.port()
        );
    }
    print_pairing(&store, &url)?;
    let app = Arc::new(App {
        store,
        socket: opts.socket,
    });
    let router = Router::new()
        .route("/api/pair", post(pair))
        .route("/ws", get(ws))
        .fallback(get(asset))
        .with_state(app);
    axum::serve(listener, router).await?;
    Ok(())
}

/// Prints a fresh pairing code as a QR code and a link.
pub fn print_pairing(store: &auth::Store, url: &str) -> Result<()> {
    let code = store.new_code()?;
    let link = format!("{}/#pair={code}", url.trim_end_matches('/'));
    match qrcode::QrCode::new(link.as_bytes()) {
        Ok(qr) => println!(
            "\n{}\n",
            qr.render::<qrcode::render::unicode::Dense1x2>()
                .quiet_zone(true)
                .build()
        ),
        Err(e) => println!("(no QR code: {e})"),
    }
    println!("  pair a phone: scan this, or open {link}");
    println!(
        "  the code works once, for {} minutes; `valk web pair` makes another",
        auth::CODE_TTL_SECS / 60
    );
    Ok(())
}

/// The tailnet HTTPS name if Tailscale runs here, else the listen address.
pub fn public_url(given: Option<String>, listen: SocketAddr) -> String {
    given
        .or_else(tailnet_url)
        .unwrap_or_else(|| format!("http://{listen}"))
}

fn tailnet_url() -> Option<String> {
    let out = std::process::Command::new("tailscale")
        .args(["status", "--json"])
        .output()
        .ok()?;
    let status: serde_json::Value = serde_json::from_slice(&out.stdout).ok()?;
    let name = status["Self"]["DNSName"].as_str()?.trim_end_matches('.');
    (!name.is_empty()).then(|| format!("https://{name}"))
}

#[derive(Deserialize)]
struct PairRequest {
    code: String,
    name: String,
}

async fn pair(State(app): State<Arc<App>>, Json(req): Json<PairRequest>) -> Response {
    match app.store.pair(&req.code, &req.name) {
        Ok(token) => {
            tracing::info!(device = req.name, "paired");
            Json(serde_json::json!({ "token": token })).into_response()
        }
        Err(e) => (StatusCode::FORBIDDEN, e.to_string()).into_response(),
    }
}

async fn ws(
    State(app): State<Arc<App>>,
    headers: HeaderMap,
    upgrade: WebSocketUpgrade,
) -> Response {
    if !same_origin(&headers) {
        return (StatusCode::FORBIDDEN, "cross-origin").into_response();
    }
    let token = headers
        .get(header::SEC_WEBSOCKET_PROTOCOL)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("")
        .split(',')
        .find_map(|p| p.trim().strip_prefix("bearer."))
        .unwrap_or("")
        .to_string();
    let Some(device) = app.store.check(&token) else {
        // Upgraded only to close with 4401: a refused handshake looks like a
        // network drop to the browser, and the app must tell "pair again" apart.
        return upgrade
            .protocols([SUBPROTOCOL])
            .on_upgrade(|mut ws| async move {
                let close = axum::extract::ws::CloseFrame {
                    code: CLOSE_UNPAIRED,
                    reason: "pair this device first".into(),
                };
                let _ = ws.send(Message::Close(Some(close))).await;
            });
    };
    let socket = app.socket.clone();
    upgrade
        .protocols([SUBPROTOCOL])
        .on_upgrade(move |ws| async move {
            if let Err(e) = bridge(ws, &socket).await {
                tracing::debug!(device = device.name, "bridge ended: {e:#}");
            }
        })
}

/// A browser's Origin must be this server, however a proxy in front names it.
fn same_origin(headers: &HeaderMap) -> bool {
    let Some(origin) = headers.get(header::ORIGIN).and_then(|v| v.to_str().ok()) else {
        // Not a browser; the token still has to match.
        return true;
    };
    let host = origin.split("://").nth(1).unwrap_or("");
    ["x-forwarded-host", "host"].iter().any(|h| {
        headers
            .get(*h)
            .and_then(|v| v.to_str().ok())
            .is_some_and(|v| v.split(',').any(|v| v.trim() == host))
    })
}

/// Messages a browser may not send: switching the daemon's binary, and posing as an
/// agent's hooks.
fn allowed(msg: &ClientMsg) -> bool {
    !matches!(msg, ClientMsg::Upgrade { .. } | ClientMsg::Hook { .. })
}

/// One daemon connection per browser connection; frames pass through as JSON.
async fn bridge(mut ws: WebSocket, socket: &std::path::Path) -> Result<()> {
    let stream = UnixStream::connect(socket)
        .await
        .with_context(|| format!("connect {}", socket.display()))?;
    let (mut from_daemon, mut to_daemon) = stream.into_split();
    let mut ping = tokio::time::interval(PING_EVERY);
    ping.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    loop {
        tokio::select! {
            msg = ws.recv() => {
                let text = match msg {
                    Some(Ok(Message::Text(text))) => text,
                    Some(Ok(Message::Close(_))) | None => return Ok(()),
                    Some(Ok(_)) => continue,
                    Some(Err(e)) => return Err(e.into()),
                };
                let value: serde_json::Value = match serde_json::from_str(&text) {
                    Ok(v) => v,
                    Err(_) => continue,
                };
                match serde_json::from_value::<ClientMsg>(value.clone()) {
                    Ok(msg) if allowed(&msg) => write_frame(&mut to_daemon, &value).await?,
                    _ => {
                        let refusal = serde_json::json!({
                            "t": "err",
                            "req": value["req"],
                            "message": "not allowed from the web app",
                        });
                        ws.send(Message::Text(refusal.to_string().into())).await?;
                    }
                }
            }
            frame = read_frame::<_, serde_json::Value>(&mut from_daemon) => {
                match frame? {
                    Some(frame) => ws.send(Message::Text(frame.to_string().into())).await?,
                    None => {
                        let _ = ws.send(Message::Close(None)).await;
                        return Ok(());
                    }
                }
            }
            _ = ping.tick() => ws.send(Message::Ping(Vec::new().into())).await?,
        }
    }
}

/// The app's files; any other path is the app itself (it routes in the browser).
async fn asset(uri: Uri) -> Response {
    let path = uri.path().trim_start_matches('/');
    let (file, path) = match DIST.get_file(path) {
        Some(file) if !path.is_empty() => (file, path),
        _ => match DIST.get_file("index.html") {
            Some(file) => (file, "index.html"),
            None => return StatusCode::NOT_FOUND.into_response(),
        },
    };
    let mut response = file.contents().into_response();
    let headers = response.headers_mut();
    headers.insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static(content_type(path)),
    );
    // Hashed build files never change; the rest must be checked on every load.
    let cache = if path.starts_with("assets/") {
        "public, max-age=31536000, immutable"
    } else {
        "no-cache"
    };
    headers.insert(header::CACHE_CONTROL, HeaderValue::from_static(cache));
    headers.insert(
        header::X_CONTENT_TYPE_OPTIONS,
        HeaderValue::from_static("nosniff"),
    );
    headers.insert(
        header::REFERRER_POLICY,
        HeaderValue::from_static("no-referrer"),
    );
    headers.insert(
        header::CONTENT_SECURITY_POLICY,
        HeaderValue::from_static(
            "default-src 'self'; connect-src 'self'; img-src 'self' data:; \
             style-src 'self' 'unsafe-inline'; frame-ancestors 'none'",
        ),
    );
    response
}

fn content_type(path: &str) -> &'static str {
    match path.rsplit('.').next().unwrap_or("") {
        "html" => "text/html; charset=utf-8",
        "js" | "mjs" => "text/javascript; charset=utf-8",
        "css" => "text/css; charset=utf-8",
        "json" => "application/json",
        "webmanifest" => "application/manifest+json",
        "svg" => "image/svg+xml",
        "png" => "image/png",
        "ico" => "image/x-icon",
        "woff2" => "font/woff2",
        "txt" => "text/plain; charset=utf-8",
        _ => "application/octet-stream",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn origin_must_be_this_server() {
        let mut h = HeaderMap::new();
        h.insert(header::HOST, HeaderValue::from_static("127.0.0.1:8790"));
        assert!(same_origin(&h), "no Origin: not a browser");
        h.insert(
            header::ORIGIN,
            HeaderValue::from_static("http://127.0.0.1:8790"),
        );
        assert!(same_origin(&h));
        h.insert(
            header::ORIGIN,
            HeaderValue::from_static("https://evil.example"),
        );
        assert!(!same_origin(&h));
        // Behind tailscale serve, the public name arrives as the forwarded host.
        h.insert(
            header::ORIGIN,
            HeaderValue::from_static("https://thor.example.ts.net"),
        );
        h.insert(
            "x-forwarded-host",
            HeaderValue::from_static("thor.example.ts.net"),
        );
        assert!(same_origin(&h));
    }

    #[test]
    fn the_web_may_not_upgrade_the_daemon_or_fake_hooks() {
        let upgrade = ClientMsg::Upgrade {
            req: 1,
            exe: "/tmp/x".into(),
        };
        assert!(!allowed(&upgrade));
        assert!(allowed(&ClientMsg::List { req: 1 }));
    }

    #[test]
    fn unknown_paths_serve_the_app() {
        assert_eq!(
            content_type("assets/index-abc.js"),
            "text/javascript; charset=utf-8"
        );
        assert!(DIST.get_file("index.html").is_some());
    }
}
