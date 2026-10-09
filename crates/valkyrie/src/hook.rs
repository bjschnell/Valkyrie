//! `valk hook <agent>` and `valk setup codex` (ADR-0005).
//!
//! The hook runs inside the agent on every event, so its contract is strict: print
//! nothing (stdout becomes model context on some events), always exit 0 (exit 2
//! blocks the agent), return in milliseconds, and do nothing outside Valkyrie.

use anyhow::{Context, Result};
use serde_json::Value;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use valkyrie_proto::ClientMsg;

/// Longest string kept from a hook payload. Tool inputs can hold whole files.
const MAX_STRING: usize = 4096;
/// Payloads larger than this are dropped rather than forwarded.
const MAX_PAYLOAD: u64 = 8 * 1024 * 1024;
const SEND_TIMEOUT: Duration = Duration::from_millis(300);

/// Never fails and never prints; errors only go to `$VALK_HOOK_LOG` if set.
pub fn run(agent: &str, socket: &Path) {
    if let Err(e) = forward(agent, socket)
        && let Some(path) = std::env::var_os("VALK_HOOK_LOG")
        && let Ok(mut log) = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(path)
    {
        let _ = writeln!(log, "valk hook {agent}: {e:#}");
    }
}

fn forward(agent: &str, socket: &Path) -> Result<()> {
    let Some(session) = std::env::var("VALK_SESSION").ok() else {
        // Not under Valkyrie (Codex hooks are global). Drain stdin anyway so the
        // agent never hits EPIPE writing the payload.
        let _ = std::io::copy(
            &mut std::io::stdin().take(MAX_PAYLOAD),
            &mut std::io::sink(),
        );
        return Ok(());
    };
    let session = session.parse().context("bad VALK_SESSION")?;
    let sent_us = SystemTime::now().duration_since(UNIX_EPOCH)?.as_micros() as u64;
    let mut raw = Vec::new();
    std::io::stdin()
        .take(MAX_PAYLOAD + 1)
        .read_to_end(&mut raw)?;
    if raw.len() as u64 > MAX_PAYLOAD {
        anyhow::bail!("payload over {MAX_PAYLOAD} bytes, dropped");
    }
    let payload = slim(serde_json::from_slice(&raw)?);
    let frame = valkyrie_proto::codec::encode(&ClientMsg::Hook {
        session,
        agent: agent.to_owned(),
        sent_us,
        payload,
    })?;
    if let Some(dir) = socket.parent() {
        valkyrie_proto::ensure_private_dir(dir)?;
    }
    // The daemon reads the whole frame before it sees EOF, so nothing to wait for.
    valkyrie_proto::ipc::exchange(socket, &frame, false, SEND_TIMEOUT)
        .with_context(|| format!("connect {}", socket.display()))?;
    Ok(())
}

/// Drops tool output and caps long strings; the final reply is kept whole because
/// its last lines are the summary.
fn slim(mut payload: Value) -> Value {
    if let Some(map) = payload.as_object_mut() {
        map.remove("tool_response");
        for (key, value) in map.iter_mut() {
            if key != "last_assistant_message" {
                cap_strings(value);
            }
        }
    }
    payload
}

fn cap_strings(value: &mut Value) {
    match value {
        Value::String(s) if s.len() > MAX_STRING => {
            let mut end = MAX_STRING;
            while !s.is_char_boundary(end) {
                end -= 1;
            }
            s.truncate(end);
            s.push('…');
        }
        Value::Array(items) => items.iter_mut().for_each(cap_strings),
        Value::Object(map) => map.values_mut().for_each(cap_strings),
        _ => {}
    }
}

/// The command line Codex runs; its hash is what `/hooks` trusts, so it must not
/// change between runs. The redirect and `|| true` keep even a stale or missing
/// binary at that path from printing or blocking the agent.
pub fn codex_command(exe: &Path) -> String {
    format!("{} hook codex 2>/dev/null || true", quote(exe))
}

/// The context hook's command line (ADR-0007), as stable as `codex_command`.
pub fn codex_context_command(exe: &Path) -> String {
    format!("{} context-hook codex 2>/dev/null || true", quote(exe))
}

/// `exe` single-quoted for a POSIX shell; with forward slashes on Windows, where
/// hooks run in Git Bash.
fn quote(exe: &Path) -> String {
    let exe = exe.to_string_lossy();
    let exe = if cfg!(windows) {
        exe.replace('\\', "/")
    } else {
        exe.into_owned()
    };
    format!("'{}'", exe.replace('\'', r"'\''"))
}

pub fn codex_hooks_path() -> PathBuf {
    let home = std::env::var_os("CODEX_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| valkyrie_proto::home_dir().join(".codex"));
    home.join("hooks.json")
}

/// Adds (or with `remove`, takes out) Valkyrie's Codex hooks. Keeps every other hook,
/// backs the file up first, and writes atomically.
pub fn setup_codex(exe: &Path, path: &Path, remove: bool, dry_run: bool) -> Result<()> {
    let command = codex_command(exe);
    let context = codex_context_command(exe);
    // Write through a symlinked hooks.json (e.g. one kept in a dotfiles repo).
    let resolved = std::fs::canonicalize(path).ok();
    let path = resolved.as_deref().unwrap_or(path);
    let existing = match std::fs::read_to_string(path) {
        Ok(text) => Some(
            serde_json::from_str::<Value>(&text)
                .with_context(|| format!("{} is not valid JSON", path.display()))?,
        ),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => None,
        Err(e) => return Err(e).with_context(|| format!("read {}", path.display())),
    };
    if remove && existing.is_none() {
        println!("{} does not exist; nothing to remove", path.display());
        return Ok(());
    }
    let doc = existing.clone().unwrap_or(Value::Null);
    let updated = if remove {
        valkyrie_agents::codex::remove_hooks(doc, &[&command, &context])
    } else {
        valkyrie_agents::codex::install_hooks(doc, &command, &context)
    };
    let text = serde_json::to_string_pretty(&updated)? + "\n";
    if dry_run {
        print!("{text}");
        return Ok(());
    }
    if existing.as_ref() == Some(&updated) {
        println!("{} already up to date", path.display());
        return Ok(());
    }
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let mode = std::fs::metadata(path).ok().map(|m| m.permissions());
    if existing.is_some() {
        let stamp = SystemTime::now().duration_since(UNIX_EPOCH)?.as_nanos();
        let backup = path.with_extension(format!("json.bak-{stamp}"));
        let mut out = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&backup)?;
        std::io::copy(&mut std::fs::File::open(path)?, &mut out)?;
        println!("backed up to {}", backup.display());
    }
    let tmp = path.with_extension(format!("json.tmp-{}", std::process::id()));
    std::fs::write(&tmp, text)?;
    if let Some(mode) = mode {
        std::fs::set_permissions(&tmp, mode)?;
    }
    std::fs::rename(&tmp, path)?;
    if remove {
        println!("removed Valkyrie hooks from {}", path.display());
    } else {
        println!("installed Valkyrie hooks in {}", path.display());
        println!("Next: start codex once and run /hooks to review and trust them.");
        println!(
            "They run `{command}` and `{context}`; moving the binary means trusting them again."
        );
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn slim_drops_tool_output_and_caps_strings_but_keeps_the_reply() {
        let long = "é".repeat(MAX_STRING);
        let p = slim(json!({
            "tool_response": "huge",
            "tool_input": {"content": long, "list": [long]},
            "last_assistant_message": long,
        }));
        assert!(p.get("tool_response").is_none());
        let capped = p["tool_input"]["content"].as_str().unwrap();
        assert!(capped.len() <= MAX_STRING + '…'.len_utf8() && capped.ends_with('…'));
        assert!(p["tool_input"]["list"][0].as_str().unwrap().ends_with('…'));
        assert_eq!(
            p["last_assistant_message"].as_str().unwrap().len(),
            long.len()
        );
    }

    #[test]
    fn codex_command_is_quoted_and_stable() {
        assert_eq!(
            codex_command(Path::new("/x/o'v/valk")),
            r"'/x/o'\''v/valk' hook codex 2>/dev/null || true"
        );
    }
}
