//! Where a session's agent keeps its own transcript, which the web app's Chat view
//! reads (DESIGN §8.7). Hooks name it (`transcript_path`). A `claude` typed at a
//! shell prompt has no hooks, since Claude's are added per session, so its
//! transcript is looked up from the process.

use serde_json::Value;
use std::path::{Path, PathBuf};

/// The transcript a hook payload names.
pub fn from_hook(payload: &Value) -> Option<PathBuf> {
    let path = PathBuf::from(payload["transcript_path"].as_str()?);
    (path.is_absolute() && path.extension().is_some_and(|e| e == "jsonl")).then_some(path)
}

/// A running Claude Code's transcript. Claude writes `sessions/<pid>.json` in its
/// config directory, naming its conversation and directory; the conversation is
/// `projects/<directory, each non-alphanumeric a dash>/<id>.jsonl`.
pub fn claude(pid: i32) -> Option<PathBuf> {
    claude_in(&claude_home()?, pid)
}

fn claude_home() -> Option<PathBuf> {
    std::env::var_os("CLAUDE_CONFIG_DIR")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".claude")))
}

fn claude_in(home: &Path, pid: i32) -> Option<PathBuf> {
    let raw = std::fs::read(home.join("sessions").join(format!("{pid}.json"))).ok()?;
    let info: Value = serde_json::from_slice(&raw).ok()?;
    // A stale file from an earlier process with this pid names another pid.
    if info["pid"].as_i64() != Some(pid as i64) {
        return None;
    }
    let id = info["sessionId"].as_str()?;
    if id.is_empty() || !id.chars().all(|c| c.is_ascii_hexdigit() || c == '-') {
        return None;
    }
    let file = format!("{id}.jsonl");
    let projects = home.join("projects");
    if let Some(cwd) = info["cwd"].as_str() {
        let path = projects.join(project_dir(cwd)).join(&file);
        if path.is_file() {
            return Some(path);
        }
    }
    // Not where its directory says (an older or newer naming): look in every project.
    std::fs::read_dir(&projects)
        .ok()?
        .flatten()
        .map(|dir| dir.path().join(&file))
        .find(|path| path.is_file())
}

/// Claude's project directory for `cwd`: `/home/u/repos/my.app` → `-home-u-repos-my-app`.
fn project_dir(cwd: &str) -> String {
    cwd.chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn a_hook_names_its_transcript() {
        let p = json!({"transcript_path": "/home/u/.claude/projects/-x/abc.jsonl"});
        assert_eq!(
            from_hook(&p),
            Some(PathBuf::from("/home/u/.claude/projects/-x/abc.jsonl"))
        );
        assert_eq!(
            from_hook(&json!({"transcript_path": "rel/abc.jsonl"})),
            None
        );
        assert_eq!(from_hook(&json!({"transcript_path": "/etc/passwd"})), None);
        assert_eq!(from_hook(&json!({})), None);
    }

    #[test]
    fn project_dirs_dash_everything_else() {
        assert_eq!(
            project_dir("/home/u/repos/valkyrie"),
            "-home-u-repos-valkyrie"
        );
        assert_eq!(project_dir("/a/3.3.5a/b_c"), "-a-3-3-5a-b-c");
    }

    #[test]
    fn finds_a_claude_transcript_from_its_pid() {
        let home = std::env::temp_dir().join(format!("valk-chat-{}", std::process::id()));
        let project = home.join("projects/-w-my-app");
        std::fs::create_dir_all(&project).unwrap();
        std::fs::create_dir_all(home.join("sessions")).unwrap();
        let id = "3064242c-ae27-48f5-8610-8c332bd6810f";
        std::fs::write(project.join(format!("{id}.jsonl")), "{}\n").unwrap();
        let write = |pid: i64, cwd: &str| {
            let info = json!({"pid": pid, "sessionId": id, "cwd": cwd});
            std::fs::write(home.join("sessions/42.json"), info.to_string()).unwrap();
        };
        write(42, "/w/my.app");
        assert_eq!(
            claude_in(&home, 42),
            Some(project.join(format!("{id}.jsonl")))
        );
        // A directory that maps elsewhere: found by its id anyway.
        write(42, "/elsewhere");
        assert_eq!(
            claude_in(&home, 42),
            Some(project.join(format!("{id}.jsonl")))
        );
        // A stale file left by another process.
        write(7, "/w/my.app");
        assert_eq!(claude_in(&home, 42), None);
        assert_eq!(claude_in(&home, 43), None);
        std::fs::remove_dir_all(&home).unwrap();
    }
}
