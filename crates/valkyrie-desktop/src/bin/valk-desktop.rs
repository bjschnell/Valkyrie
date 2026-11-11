//! The desktop app (ADR-0020): a native window around the web app that `valk web`
//! serves on this machine, a tray icon with the number of sessions that need you,
//! and native notifications for them. Everything inside the window is the web
//! app; what the window stands on is this crate's library.

// No console window beside the app on Windows.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use tauri::menu::{CheckMenuItem, Menu, MenuItem};
use tauri::tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent};
use tauri::webview::NewWindowResponse;
use tauri::{AppHandle, Manager, Url, WebviewUrl, WebviewWindowBuilder, WindowEvent};
use tauri_plugin_autostart::ManagerExt as _;
use tauri_plugin_dialog::{DialogExt as _, MessageDialogButtons, MessageDialogKind};
use tauri_plugin_notification::NotificationExt as _;
use tauri_plugin_opener::OpenerExt as _;
use valkyrie_desktop::pair::Pairing;
use valkyrie_desktop::start::{self, Valk};
use valkyrie_desktop::watch::{self, Event};

const WINDOW: &str = "main";
const TRAY: &str = "valkyrie";
const NAME: &str = "Valkyrie";

/// Set before the page's own scripts run: the page then leaves notifications to
/// the app, and tells it when it isn't paired (`PAIR_SIGNAL`).
const PAGE_FLAG: &str = "window.__VALK_DESKTOP__ = true;";
/// Where the page goes to say it isn't paired. Never loaded: the window's
/// navigation is where the app hears of it. A page can say only this, so it has
/// no way to call into the app.
const PAIR_SIGNAL: &str = "valk-desktop";

/// What the launch knows once `valk web` answered.
struct Shell {
    addr: SocketAddr,
    socket: PathBuf,
    pairing: Pairing,
    /// The pairing dialog is up: a page that asks again gets no second one.
    asking: AtomicBool,
}

fn main() {
    // To stderr, which a terminal shows and a dock throws away. `RUST_LOG` picks
    // how much.
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "warn,valk_desktop=info,valkyrie_desktop=info".into()),
        )
        .with_writer(std::io::stderr)
        .init();
    tauri::Builder::default()
        // First, so a second launch is over before it has done anything else.
        .plugin(tauri_plugin_single_instance::init(|app, _args, _cwd| {
            show(app)
        }))
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_notification::init())
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_autostart::Builder::new().build())
        .setup(|app| {
            // On Linux the tray's library is loaded as the icon is made, and a
            // machine without it panics there. The app is worth having without
            // a tray: a second launch still brings the window back.
            let handle = app.handle().clone();
            match std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| tray(&handle))) {
                Ok(Ok(())) => {}
                Ok(Err(e)) => tracing::warn!("no tray icon: {e}"),
                Err(_) => tracing::warn!("no tray icon: its library isn't installed"),
            }
            let app = app.handle().clone();
            // Finding `valk web`, or setting it up, can take a while: the tray is
            // there meanwhile, and the window comes when there is a page for it.
            tauri::async_runtime::spawn(async move {
                let valk = Valk::find();
                tracing::info!(valk = %valk.exe.display(), "looking for valk web");
                match start::ensure(&valk).await {
                    Ok(addr) => {
                        tracing::info!(%addr, "valk web is yours; opening the window");
                        started(app, addr)
                    }
                    Err(e) => {
                        tracing::error!("{e}");
                        fail(&app, &e.to_string())
                    }
                }
            });
            Ok(())
        })
        .build(tauri::generate_context!())
        .expect("the desktop app failed to start")
        // Closing the window leaves the tray, its count and its notifications.
        .run(|app, event| {
            // A click on the Dock icon, with the window closed.
            #[cfg(target_os = "macos")]
            if let tauri::RunEvent::Reopen { .. } = event {
                show(app);
            }
            #[cfg(not(target_os = "macos"))]
            let _ = (app, event);
        });
}

/// `valk web` answers on `addr` and proved it's the user's: open the window on
/// it, and start following the daemon.
fn started(app: AppHandle, addr: SocketAddr) {
    let socket = valkyrie_proto::daemon_socket(std::env::var_os("VALK_SOCKET").map(PathBuf::from));
    app.manage(Shell {
        addr,
        socket: socket.clone(),
        pairing: Pairing::new(),
        asking: AtomicBool::new(false),
    });
    let opener = app.clone();
    let opened = app.run_on_main_thread(move || {
        if let Err(e) = window(&opener, addr) {
            fail(&opener, &format!("Couldn't open the window: {e}"));
        }
    });
    if let Err(e) = opened {
        return fail(&app, &format!("Couldn't open the window: {e}"));
    }
    let (events, mut seen) = tokio::sync::mpsc::unbounded_channel();
    let watcher = app.clone();
    tauri::async_runtime::spawn(async move {
        let front = || in_front(&watcher);
        watch::run(&socket, front, events).await;
    });
    tauri::async_runtime::spawn(async move {
        while let Some(event) = seen.recv().await {
            told(&app, event);
        }
    });
}

fn window(app: &AppHandle, addr: SocketAddr) -> tauri::Result<()> {
    let url: Url = start::url(addr).parse().expect("a local http address");
    let (navigator, opener) = (app.clone(), app.clone());
    let window = WebviewWindowBuilder::new(app, WINDOW, WebviewUrl::External(url))
        .title(NAME)
        .inner_size(1280.0, 860.0)
        .min_inner_size(420.0, 480.0)
        .initialization_script(PAGE_FLAG)
        .on_navigation(move |to| {
            if to.scheme() == PAIR_SIGNAL {
                tracing::info!("the page isn't paired; asking at the window");
                ask_to_pair(&navigator);
                return false;
            }
            if ours(to, addr) {
                return true;
            }
            // A link out of the app belongs in the browser, not in this window.
            outside(&navigator, to);
            false
        })
        .on_new_window(move |to, _features| {
            if !ours(&to, addr) {
                outside(&opener, &to);
            }
            NewWindowResponse::Deny
        })
        .build()?;
    let hidden = window.clone();
    window.on_window_event(move |event| {
        if let WindowEvent::CloseRequested { api, .. } = event {
            api.prevent_close();
            let _ = hidden.hide();
        }
    });
    Ok(())
}

/// Hands a web link to the browser. Only a web link: any other scheme would go
/// to whatever the system has for it (a file, a network share, a program's own
/// handler), with no browser in between to ask.
fn outside(app: &AppHandle, url: &Url) {
    if !matches!(url.scheme(), "http" | "https") {
        tracing::warn!(
            scheme = url.scheme(),
            "not opening a link that isn't a web address"
        );
        return;
    }
    if let Err(e) = app.opener().open_url(url.as_str(), None::<&str>) {
        tracing::warn!("open {url}: {e}");
    }
}

/// The web app's own pages: the one address the window was opened on.
fn ours(url: &Url, addr: SocketAddr) -> bool {
    url.scheme() == "http"
        && url.host_str() == Some("127.0.0.1")
        && url.port_or_known_default() == Some(addr.port())
}

/// The window is on screen and has the keyboard: what it shows has been seen.
fn in_front(app: &AppHandle) -> bool {
    app.get_webview_window(WINDOW)
        .is_some_and(|w| w.is_visible().unwrap_or(false) && w.is_focused().unwrap_or(false))
}

fn show(app: &AppHandle) {
    if let Some(window) = app.get_webview_window(WINDOW) {
        let _ = window.unminimize();
        let _ = window.show();
        let _ = window.set_focus();
    }
}

/// The page says it isn't paired. Pairing is a person's to allow, and pressing
/// Pair here is how: only then is the daemon asked, and it answers a launch once
/// (`Pairing`).
fn ask_to_pair(app: &AppHandle) {
    let Some(shell) = app.try_state::<Shell>() else {
        return;
    };
    if shell.asking.swap(true, Ordering::SeqCst) {
        return;
    }
    let app = app.clone();
    app.clone()
        .dialog()
        .message(
            "Pair this app with Valkyrie on this computer?\n\n\
             It can then do what you do in a terminal: start agents, answer them \
             and type into their sessions.",
        )
        .title(NAME)
        .buttons(MessageDialogButtons::OkCancelCustom(
            "Pair".into(),
            "Not now".into(),
        ))
        .show(move |pair| {
            tauri::async_runtime::spawn(async move {
                if pair {
                    paired(&app).await;
                }
                if let Some(shell) = app.try_state::<Shell>() {
                    shell.asking.store(false, Ordering::SeqCst);
                }
            });
        });
}

async fn paired(app: &AppHandle) {
    let Some(shell) = app.try_state::<Shell>() else {
        return;
    };
    let link = shell
        .pairing
        .link(&shell.socket, &start::url(shell.addr))
        .await;
    match link {
        Ok(Some(link)) => {
            tracing::info!("the daemon vouched; loading the pairing link");
            let Some(window) = app.get_webview_window(WINDOW) else {
                return;
            };
            // The page reads the code as it loads, so it loads again with it.
            let link = serde_json::to_string(&link).unwrap_or_default();
            let _ = window.eval(format!("location.replace({link}); location.reload();"));
        }
        // This launch had its answer: the page's own pairing screen is what's left.
        Ok(None) => {
            app.dialog()
                .message(
                    "This app already asked to pair since it was opened. Quit it from the \
                     tray and open it again, or type a code from `valk web pair`.",
                )
                .title(NAME)
                .show(|_| {});
        }
        Err(e) => {
            tracing::warn!("pairing: {e:#}");
            app.dialog()
                .message(format!("Couldn't pair this app: {e:#}"))
                .title(NAME)
                .kind(MessageDialogKind::Error)
                .show(|_| {});
        }
    }
}

/// The tray: the count beside the icon, the window on a click, and a menu.
fn tray(app: &AppHandle) -> tauri::Result<()> {
    let open = MenuItem::with_id(app, "open", "Open Valkyrie", true, None::<&str>)?;
    let login = CheckMenuItem::with_id(
        app,
        "login",
        "Start at login",
        true,
        app.autolaunch().is_enabled().unwrap_or(false),
        None::<&str>,
    )?;
    let quit = MenuItem::with_id(app, "quit", "Quit", true, None::<&str>)?;
    let menu = Menu::with_items(app, &[&open, &login, &quit])?;
    let icon = tauri::image::Image::from_bytes(include_bytes!("../../icons/tray.png"))?;
    TrayIconBuilder::with_id(TRAY)
        .icon(icon)
        .tooltip(format!("{NAME}: starting"))
        .menu(&menu)
        .show_menu_on_left_click(false)
        .on_menu_event(move |app, event| match event.id().as_ref() {
            "open" => show(app),
            "login" => {
                let launch = app.autolaunch();
                let on = launch.is_enabled().unwrap_or(false);
                let changed = if on {
                    launch.disable()
                } else {
                    launch.enable()
                };
                if let Err(e) = changed {
                    tracing::warn!("start at login: {e}");
                }
                let _ = login.set_checked(launch.is_enabled().unwrap_or(false));
            }
            "quit" => app.exit(0),
            _ => {}
        })
        .on_tray_icon_event(|tray, event| {
            if let TrayIconEvent::Click {
                button: MouseButton::Left,
                button_state: MouseButtonState::Up,
                ..
            } = event
            {
                show(tray.app_handle());
            }
        })
        .build(app)?;
    Ok(())
}

/// What the daemon's watcher said, on the tray and as a notification.
fn told(app: &AppHandle, event: Event) {
    let tray = app.tray_by_id(TRAY);
    let label = |tooltip: String, title: Option<String>| {
        if let Some(tray) = &tray {
            let _ = tray.set_tooltip(Some(tooltip));
            let _ = tray.set_title(title);
        }
    };
    tracing::debug!(?event, "from the daemon");
    match event {
        Event::Online(true) => {}
        Event::Online(false) => label(format!("{NAME}: no daemon"), None),
        Event::Outdated => label(
            format!("{NAME} was upgraded: quit and open this app again"),
            Some("!".into()),
        ),
        Event::Count(0) => label(format!("{NAME}: nothing needs you"), None),
        Event::Count(n) => label(format!("{NAME}: {n} need you"), Some(n.to_string())),
        Event::Notify(note) => {
            let shown = app
                .notification()
                .builder()
                .title(note.title)
                .body(note.body)
                .show();
            if let Err(e) = shown {
                tracing::warn!("notification: {e}");
            }
        }
    }
}

/// The app can't show the web app: say why, then leave.
fn fail(app: &AppHandle, why: &str) {
    let quit = app.clone();
    app.dialog()
        .message(why)
        .title(NAME)
        .kind(MessageDialogKind::Error)
        .show(move |_| quit.exit(1));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_the_web_apps_own_address_loads_in_the_window() {
        let addr: SocketAddr = "127.0.0.1:8790".parse().unwrap();
        let at = |url: &str| ours(&url.parse().unwrap(), addr);
        assert!(at("http://127.0.0.1:8790/"));
        assert!(at("http://127.0.0.1:8790/#pair=abc"));
        assert!(at("http://127.0.0.1:8790/assets/index.js"));
        // Another port, another spelling, another scheme, another machine: the
        // browser's, not the window's.
        assert!(!at("http://127.0.0.1:8791/"));
        assert!(!at("http://localhost:8790/"));
        assert!(!at("https://127.0.0.1:8790/"));
        assert!(!at("http://127.0.0.1.example.com:8790/"));
        assert!(!at("http://user@example.com:8790/"));
        assert!(!at("https://github.com/bjschnell/Valkyrie"));
        assert!(!at("valk-desktop://pair"));
        assert!(!at("file:///etc/passwd"));
    }
}
