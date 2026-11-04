//! `valk hook <agent>` and `valk setup codex` (ADR-0005).
//!
//! The hook runs inside the agent on every event, so its contract is strict: print
//! nothing (stdout becomes model context on some events), always exit 0 (exit 2
//! blocks the agent), return in milliseconds, and do nothing outside Valkyrie.
//!
//! One exception, by the user's own accepted policy (ADR-0013 §5): on Claude's
//! `Stop`, when an active `ship` policy finds the work short of where the user
//! ships it, or `ci-fix` finds its pull request's checks failed, the hook prints
//! Claude's block with the policy's text, and tells the daemon the turn goes on. It's this hook rather than the context hook so the one
//! process that reports the stop also decides it: the queue never shows a turn
//! end the agent didn't take.
//!
//! The other: the merge guard (ADR-0015 §1). On `PreToolUse`, a shell command that
//! merges a pull request or pushes to the default branch gets Claude's "ask", a
//! confirm the user answers at their own terminal, or Codex's refusal.

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
/// Longest a policy waits on `gh` for a pull request: Claude gives the
/// hook 5 s, and an unanswered lookup lets the stop through.
const GH_TIMEOUT: Duration = Duration::from_secs(3);

/// Never fails; prints only a policy's block (see the module doc); errors only go
/// to `$VALK_HOOK_LOG` if set.
pub fn run(agent: &str, socket: &Path, ambient: bool) {
    // The session was launched with this hook in its own arguments.
    if ambient && std::env::var_os("VALK_SESSION_HOOKS").is_some() {
        let _ = std::io::copy(
            &mut std::io::stdin().take(MAX_PAYLOAD),
            &mut std::io::sink(),
        );
        return;
    }
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
    let mut payload: Value = serde_json::from_slice(&raw)?;
    // Before `slim`, which cuts long commands short.
    let guard = merge_guard(agent, &mut payload, &crate::guard::Git);
    let mut payload = slim(payload);
    let block = (agent == "claude")
        .then(|| policy_block(&mut payload))
        .flatten();
    let sent = send(socket, session, agent, sent_us, payload);
    // Blocked whether or not the daemon heard: the policy is the user's either way.
    if let Some(reason) = block {
        println!(
            "{}",
            serde_json::json!({"decision": "block", "reason": reason})
        );
    }
    if let Some(decision) = guard {
        println!("{decision}");
    }
    sent
}

/// On a shell `PreToolUse`, the merge guard's answer (ADR-0015 §1): Claude's "ask",
/// so the user approves at their own terminal, or Codex's "deny", which has no
/// confirm of its own. The payload is marked so the daemon knows the confirm on
/// screen is the guard's and keeps other agents from answering it.
fn merge_guard(agent: &str, payload: &mut Value, repo: &dyn crate::guard::Repo) -> Option<Value> {
    if payload["hook_event_name"] != "PreToolUse"
        || !matches!(
            payload["tool_name"].as_str(),
            Some("Bash" | "PowerShell" | "shell")
        )
    {
        return None;
    }
    let command = payload["tool_input"]["command"].as_str()?;
    let cwd = PathBuf::from(payload["cwd"].as_str()?);
    let hit = crate::guard::check(command, &cwd, repo)?;
    let (decision, reason) = match agent {
        "claude" => (
            "ask",
            format!(
                "Valkyrie merge guard: {} Merging is yours (ADR-0015): approve only if you asked for it.",
                hit.what()
            ),
        ),
        _ => (
            "deny",
            format!(
                "{} Merging is the user's (ADR-0015): don't merge or push to the default branch \
                 yourself. Tell the user the pull request is ready instead.",
                hit.what()
            ),
        ),
    };
    payload["valk_merge_guard"] = serde_json::json!({"kind": hit.kind(), "decision": decision});
    Some(serde_json::json!({
        "hookSpecificOutput": {
            "hookEventName": "PreToolUse",
            "permissionDecision": decision,
            "permissionDecisionReason": reason,
        }
    }))
}

fn send(
    socket: &Path,
    session: valkyrie_proto::SessionId,
    agent: &str,
    sent_us: u64,
    payload: Value,
) -> Result<()> {
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

/// On a main-thread `Stop`, the user's accepted policies (ADR-0013): the text to
/// keep the turn going, with the payload marked so the daemon doesn't count a turn
/// end. `ship` (the user's habit with the project's settings over it, ADR-0016)
/// answers work short of where the user ships it; `ci-fix`, on where the
/// repository's `ci: gh` is accepted, answers a pull request whose checks failed,
/// at most [`policy::CI_FIX_LIMIT`] times per pull request. Once per turn: Claude
/// says `stop_hook_active` when the stop follows a block.
fn policy_block(payload: &mut Value) -> Option<String> {
    use valkyrie_context::policy;
    use valkyrie_daemon::work;
    if payload["hook_event_name"] != "Stop"
        || payload["stop_hook_active"] == true
        || !payload["agent_id"].is_null()
        || !valkyrie_tui::context_enabled()
    {
        return None;
    }
    let cwd = PathBuf::from(payload["cwd"].as_str()?);
    let root = valkyrie_context::project::root(&cwd);
    let store = valkyrie_context::Store::new(crate::context::store_base());
    let (project, user) = (
        store.load(&root),
        store.load(&valkyrie_context::project::user()),
    );
    let ship = policy::ship(&project, &user);
    let ci = policy::ci(&project).filter(|(ci, _)| *ci == policy::Ci::Gh);
    if ship.is_none() && ci.is_none() {
        return None;
    }
    // Nothing past a push matters unless a policy looks at the pull request.
    let ask = ci.is_some()
        || ship
            .as_ref()
            .is_some_and(|s| s.until.0 == policy::Until::Pr);
    let seen = std::cell::RefCell::new(None);
    let pr = |dir: &Path, branch: &str| {
        if !ask {
            return work::Pr::Unknown;
        }
        let pr = work::gh_pr(dir, branch, GH_TIMEOUT);
        *seen.borrow_mut() = Some(pr.clone());
        pr
    };
    let (state, branch, url) = work::state(&cwd, &pr)?;
    let branch = branch.unwrap_or_default();
    let named = |source: policy::Source| match source {
        policy::Source::User(id) => (id, "user"),
        policy::Source::Project(id) => (id, "project"),
    };
    if let Some(ship) = ship
        && policy::short(&ship, state.as_str())
    {
        let (policy, scope) = named(ship.until.1);
        payload["valk_continued"] = serde_json::json!({"template": "ship", "policy": policy,
            "scope": scope, "state": state.as_str(), "until": ship.until.0.as_str()});
        return Some(policy::reason(&ship, state.as_str(), &branch));
    }
    let (_, source) = ci?;
    if state != work::State::PrRed {
        return None;
    }
    let url = url?;
    let Some(work::Pr::Open { failed, .. }) = seen.take() else {
        return None;
    };
    let now = SystemTime::now().duration_since(UNIX_EPOCH).ok()?.as_secs();
    // Past the limit the stop goes through: what's left is the user's. Uncounted
    // (the store can't be written), it could go on forever: no block either.
    let block = store
        .record_ci_fix(&root, &url, now, policy::CI_FIX_LIMIT)
        .ok()??;
    let (policy, scope) = named(source);
    payload["valk_continued"] = serde_json::json!({"template": "ci-fix", "policy": policy,
        "scope": scope, "state": state.as_str(), "pr": url, "block": block});
    Some(policy::ci_fix(
        source,
        policy::checks(&project).as_ref(),
        &url,
        &branch,
        &failed,
        block,
    ))
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

/// Claude Code's user settings: `$CLAUDE_CONFIG_DIR/settings.json`, else
/// `~/.claude/settings.json`.
pub fn claude_settings_path() -> PathBuf {
    let home = std::env::var_os("CLAUDE_CONFIG_DIR")
        .map(PathBuf::from)
        .filter(|p| p.is_absolute())
        .unwrap_or_else(|| valkyrie_proto::home_dir().join(".claude"));
    home.join("settings.json")
}

/// Puts Valkyrie's hooks in Claude Code's own settings, or takes them out, to match
/// the Context setting. It is how a `claude` typed in a Valkyrie shell (which
/// Valkyrie never launched, so could not hand `--settings`) still gets the
/// project's decisions, and still has its corrections read and its edits told to
/// the other agents. Best effort: a settings file that can't be edited costs
/// the feature there, not the caller anything.
///
/// A debug build (the test suite's daemons) leaves the real settings alone unless
/// `$CLAUDE_CONFIG_DIR` names where to put the hook.
pub fn sync_claude(on: bool) {
    if cfg!(debug_assertions) && std::env::var_os("CLAUDE_CONFIG_DIR").is_none() {
        return;
    }
    let Ok(exe) = std::env::current_exe() else {
        return;
    };
    let path = claude_settings_path();
    // Not installing into a Claude Code that was never set up here.
    if on && !path.parent().is_some_and(Path::is_dir) {
        return;
    }
    if let Err(e) = setup_claude(&exe, &path, !on, false, true) {
        tracing::warn!("claude settings not updated: {e:#}");
    }
}

/// Adds (or with `remove`, takes out) the ambient hooks in Claude Code's settings. Keeps everything else, writes atomically, and keeps one backup of the
/// file as it was before Valkyrie first changed it. `quiet` prints nothing.
pub fn setup_claude(
    exe: &Path,
    path: &Path,
    remove: bool,
    dry_run: bool,
    quiet: bool,
) -> Result<()> {
    let say = |text: String| {
        if !quiet {
            println!("{text}");
        }
    };
    // Write through a symlinked settings.json (e.g. one kept in a dotfiles repo).
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
        say(format!(
            "{} does not exist; nothing to remove",
            path.display()
        ));
        return Ok(());
    }
    let doc = existing.clone().unwrap_or(Value::Null);
    let updated = if remove {
        valkyrie_agents::remove_ambient(doc)
    } else {
        valkyrie_agents::install_ambient(doc, exe)
    };
    let text = serde_json::to_string_pretty(&updated)? + "\n";
    if dry_run {
        print!("{text}");
        return Ok(());
    }
    if existing.as_ref() == Some(&updated) {
        say(format!("{} already up to date", path.display()));
        return Ok(());
    }
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let mode = std::fs::metadata(path).ok().map(|m| m.permissions());
    if existing.is_some() {
        let backup = path.with_extension("json.valkyrie-bak");
        if !backup.exists() {
            std::fs::copy(path, &backup)?;
            say(format!("backed up to {}", backup.display()));
        }
    }
    let tmp = path.with_extension(format!("json.tmp-{}", std::process::id()));
    std::fs::write(&tmp, text)?;
    if let Some(mode) = mode {
        std::fs::set_permissions(&tmp, mode)?;
    }
    std::fs::rename(&tmp, path)?;
    if remove {
        say(format!("removed Valkyrie's hooks from {}", path.display()));
    } else {
        say(format!("installed Valkyrie's hooks in {}", path.display()));
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

    struct OnMain;

    impl crate::guard::Repo for OnMain {
        fn default_branch(&self, _: &Path) -> Option<String> {
            Some("main".into())
        }
        fn current_branch(&self, _: &Path) -> Option<String> {
            Some("feat/x".into())
        }
    }

    fn pre_tool(command: &str) -> Value {
        json!({"hook_event_name": "PreToolUse", "tool_name": "Bash", "cwd": "/repo",
               "tool_input": {"command": command}})
    }

    #[test]
    fn the_merge_guard_asks_claude_and_marks_the_payload() {
        let mut payload = pre_tool("gh pr merge 81 --squash");
        let out = merge_guard("claude", &mut payload, &OnMain).unwrap();
        let answer = &out["hookSpecificOutput"];
        assert_eq!(answer["hookEventName"], "PreToolUse");
        assert_eq!(answer["permissionDecision"], "ask");
        assert!(
            answer["permissionDecisionReason"]
                .as_str()
                .unwrap()
                .contains("merges a pull request")
        );
        assert_eq!(
            payload["valk_merge_guard"],
            json!({"kind": "merge", "decision": "ask"})
        );
    }

    #[test]
    fn the_merge_guard_refuses_codex() {
        let mut payload = pre_tool("git push origin HEAD:main");
        let out = merge_guard("codex", &mut payload, &OnMain).unwrap();
        let answer = &out["hookSpecificOutput"];
        assert_eq!(answer["permissionDecision"], "deny");
        let reason = answer["permissionDecisionReason"].as_str().unwrap();
        assert!(reason.contains("default branch (main)") && reason.contains("is ready"));
    }

    #[test]
    fn the_merge_guard_leaves_everything_else_alone() {
        for mut payload in [
            pre_tool("git push -u origin feat/x"),
            json!({"hook_event_name": "PostToolUse", "tool_name": "Bash", "cwd": "/repo",
                   "tool_input": {"command": "gh pr merge 81"}}),
            json!({"hook_event_name": "PreToolUse", "tool_name": "Write", "cwd": "/repo",
                   "tool_input": {"command": "gh pr merge 81"}}),
        ] {
            assert!(merge_guard("claude", &mut payload, &OnMain).is_none());
            assert!(payload.get("valk_merge_guard").is_none());
        }
    }

    #[test]
    fn the_merge_guard_reads_the_whole_command_before_slim_cuts_it() {
        let long = format!("echo {} && gh pr merge 81", "x".repeat(MAX_STRING));
        let mut payload = pre_tool(&long);
        assert!(merge_guard("claude", &mut payload, &OnMain).is_some());
    }

    #[test]
    fn codex_command_is_quoted_and_stable() {
        assert_eq!(
            codex_command(Path::new("/x/o'v/valk")),
            r"'/x/o'\''v/valk' hook codex 2>/dev/null || true"
        );
    }
}
