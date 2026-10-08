//! The agent running in a shell session's foreground. A `claude` typed at the prompt
//! got no hooks, but the session can still show its name and read its screen.

use std::collections::VecDeque;
use std::path::Path;

/// Processes looked at per check, so a fork bomb in the foreground costs nothing.
const MAX_PROCS: usize = 64;

/// The agent in foreground process group `pgrp`: the one nearest the group's
/// leader, which may be a wrapper script that started it.
pub fn agent(pgrp: i32) -> Option<&'static str> {
    let mut queue = VecDeque::from([pgrp]);
    let mut looked = 0;
    while let Some(pid) = queue.pop_front() {
        looked += 1;
        if looked > MAX_PROCS {
            break;
        }
        if let Some(agent) = cmdline(pid).and_then(|argv| agent_of(&argv)) {
            return Some(agent);
        }
        // Background jobs a wrapper started are in other groups.
        queue.extend(
            children(pid)
                .into_iter()
                .filter(|&child| group_of(child) == Some(pgrp)),
        );
    }
    None
}

/// The agent a command line runs: its program, or the script a JS runtime runs
/// (`node …/bin/codex.js`). Other programs' arguments never count (`vim claude.md`).
fn agent_of(argv: &[String]) -> Option<&'static str> {
    let stem = |arg: &String| {
        Path::new(arg)
            .file_stem()
            .and_then(|s| s.to_str())
            .map(str::to_owned)
    };
    let known = |name: &str| {
        let adapter = valkyrie_agents::by_name(name);
        (adapter.name() != "generic").then(|| adapter.name())
    };
    let program = stem(argv.first()?)?;
    if let Some(agent) = known(&program) {
        return Some(agent);
    }
    match program.as_str() {
        "node" | "bun" | "deno" => known(&stem(argv.get(1)?)?),
        _ => None,
    }
}

fn cmdline(pid: i32) -> Option<Vec<String>> {
    let raw = std::fs::read(format!("/proc/{pid}/cmdline")).ok()?;
    Some(
        raw.split(|&b| b == 0)
            .filter(|a| !a.is_empty())
            .take(2)
            .map(|a| String::from_utf8_lossy(a).into_owned())
            .collect(),
    )
}

/// Children of every thread (a JS runtime spawns from worker threads).
fn children(pid: i32) -> Vec<i32> {
    let Ok(tasks) = std::fs::read_dir(format!("/proc/{pid}/task")) else {
        return Vec::new();
    };
    tasks
        .flatten()
        .filter_map(|task| std::fs::read_to_string(task.path().join("children")).ok())
        .flat_map(|list| {
            list.split_whitespace()
                .filter_map(|p| p.parse().ok())
                .collect::<Vec<_>>()
        })
        .collect()
}

/// Field 5 of `/proc/<pid>/stat`, counted after the parenthesized command name,
/// which may itself hold spaces and parentheses.
fn group_of(pid: i32) -> Option<i32> {
    let stat = std::fs::read_to_string(format!("/proc/{pid}/stat")).ok()?;
    let rest = &stat[stat.rfind(')')? + 1..];
    rest.split_whitespace().nth(2)?.parse().ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn argv(args: &[&str]) -> Vec<String> {
        args.iter().map(|a| a.to_string()).collect()
    }

    #[test]
    fn knows_agents_by_program_or_runtime_script() {
        assert_eq!(agent_of(&argv(&["claude", "-p"])), Some("claude"));
        assert_eq!(agent_of(&argv(&["/usr/bin/codex"])), Some("codex"));
        assert_eq!(
            agent_of(&argv(&[
                "node",
                "/usr/lib/node_modules/@openai/codex/bin/codex.js"
            ])),
            Some("codex")
        );
        assert_eq!(agent_of(&argv(&["vim", "claude.md"])), None);
        assert_eq!(agent_of(&argv(&["fish"])), None);
        assert_eq!(agent_of(&[]), None);
    }

    /// A wrapper script in the foreground, the agent its child: found below it.
    #[test]
    fn finds_an_agent_under_a_wrapper() {
        use std::os::unix::process::CommandExt;
        let dir = std::env::temp_dir().join(format!("valkyrie-fg-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        // `sleep` under the name `claude`: argv[0] keeps the name it was started by.
        let fake = dir.join("claude");
        let _ = std::fs::remove_file(&fake);
        std::os::unix::fs::symlink("/bin/sleep", &fake).unwrap();
        // `sh -c` in its own group, like a job the shell put in the foreground.
        let mut wrapper = std::process::Command::new("sh")
            .arg("-c")
            .arg(format!("{} 30 & wait", fake.display()))
            .process_group(0)
            .spawn()
            .unwrap();
        let pgrp = wrapper.id() as i32;
        let mut found = None;
        for _ in 0..50 {
            found = agent(pgrp);
            if found.is_some() {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
        // SAFETY: signals the group this test made.
        unsafe { libc::kill(-pgrp, libc::SIGKILL) };
        let _ = wrapper.wait();
        std::fs::remove_dir_all(&dir).unwrap();
        assert_eq!(found, Some("claude"));
    }
}
