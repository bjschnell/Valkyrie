//! The agent running in a shell session's foreground. A `claude` typed at the prompt
//! got no hooks, but the session can still show its name and read its screen.

use std::collections::VecDeque;
use std::path::Path;

/// Processes looked at per check, so a fork bomb in the foreground costs nothing.
const MAX_PROCS: usize = 64;

/// The agent in foreground process group `pgrp`, and its pid: the one nearest the
/// group's leader, which may be a wrapper script that started it.
pub fn agent(pgrp: i32) -> Option<(&'static str, i32)> {
    let mut queue = VecDeque::from([pgrp]);
    let mut looked = 0;
    while let Some(pid) = queue.pop_front() {
        looked += 1;
        if looked > MAX_PROCS {
            break;
        }
        if let Some(agent) = cmdline(pid).and_then(|argv| agent_of(&argv)) {
            return Some((agent, pid));
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

pub use sys::cwd;
use sys::{children, cmdline, group_of};

/// Linux: everything is in `/proc`.
#[cfg(target_os = "linux")]
mod sys {
    use std::path::PathBuf;

    /// A process's working directory.
    pub fn cwd(pid: i32) -> Option<PathBuf> {
        std::fs::read_link(format!("/proc/{pid}/cwd")).ok()
    }

    /// The first two arguments.
    pub fn cmdline(pid: i32) -> Option<Vec<String>> {
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
    pub fn children(pid: i32) -> Vec<i32> {
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
    pub fn group_of(pid: i32) -> Option<i32> {
        let stat = std::fs::read_to_string(format!("/proc/{pid}/stat")).ok()?;
        let rest = &stat[stat.rfind(')')? + 1..];
        rest.split_whitespace().nth(2)?.parse().ok()
    }
}

/// macOS: libproc and `sysctl`, as tmux does it.
#[cfg(target_os = "macos")]
mod sys {
    use std::ffi::CStr;
    use std::path::PathBuf;

    pub fn cwd(pid: i32) -> Option<PathBuf> {
        // SAFETY: zeroed is a valid proc_vnodepathinfo; proc_pidinfo fills at most
        // the size given.
        let mut info: libc::proc_vnodepathinfo = unsafe { std::mem::zeroed() };
        let size = std::mem::size_of::<libc::proc_vnodepathinfo>() as libc::c_int;
        let n = unsafe {
            libc::proc_pidinfo(
                pid,
                libc::PROC_PIDVNODEPATHINFO,
                0,
                (&mut info as *mut libc::proc_vnodepathinfo).cast(),
                size,
            )
        };
        if n != size {
            return None;
        }
        // SAFETY: the kernel NUL-terminates the path inside the array.
        let path = unsafe { CStr::from_ptr(info.pvi_cdir.vip_path.as_ptr().cast()) };
        let path = path.to_str().ok()?;
        (!path.is_empty()).then(|| PathBuf::from(path))
    }

    /// The first two arguments, from `KERN_PROCARGS2`: argc, the executable path,
    /// padding, then argv.
    pub fn cmdline(pid: i32) -> Option<Vec<String>> {
        let mut mib = [libc::CTL_KERN, libc::KERN_PROCARGS2, pid];
        let mut size: libc::size_t = 0;
        // SAFETY: a size query with no buffer.
        let ok = unsafe {
            libc::sysctl(
                mib.as_mut_ptr(),
                3,
                std::ptr::null_mut(),
                &mut size,
                std::ptr::null_mut(),
                0,
            )
        };
        if ok != 0 || size < 4 {
            return None;
        }
        let mut buf = vec![0u8; size];
        // SAFETY: `buf` holds `size` bytes.
        let ok = unsafe {
            libc::sysctl(
                mib.as_mut_ptr(),
                3,
                buf.as_mut_ptr().cast(),
                &mut size,
                std::ptr::null_mut(),
                0,
            )
        };
        if ok != 0 {
            return None;
        }
        buf.truncate(size);
        let rest = &buf[4..];
        // Past the executable path and the NULs after it.
        let start = rest.iter().position(|&b| b == 0)?;
        let rest = &rest[start..];
        let args = &rest[rest.iter().position(|&b| b != 0)?..];
        Some(
            args.split(|&b| b == 0)
                .take(2)
                .map(|a| String::from_utf8_lossy(a).into_owned())
                .collect(),
        )
    }

    pub fn children(pid: i32) -> Vec<i32> {
        let mut pids = vec![0 as libc::pid_t; 256];
        let bytes = (pids.len() * std::mem::size_of::<libc::pid_t>()) as libc::c_int;
        // SAFETY: the buffer holds `bytes` bytes.
        let n = unsafe { libc::proc_listchildpids(pid, pids.as_mut_ptr().cast(), bytes) };
        pids.truncate(n.max(0) as usize);
        pids
    }

    pub fn group_of(pid: i32) -> Option<i32> {
        // SAFETY: getpgid only reads.
        let group = unsafe { libc::getpgid(pid) };
        (group > 0).then_some(group)
    }
}

/// Elsewhere: no agent detection; sessions keep their spawn directory.
#[cfg(not(any(target_os = "linux", target_os = "macos")))]
mod sys {
    pub fn cwd(_: i32) -> Option<std::path::PathBuf> {
        None
    }
    pub fn cmdline(_: i32) -> Option<Vec<String>> {
        None
    }
    pub fn children(_: i32) -> Vec<i32> {
        Vec::new()
    }
    pub fn group_of(_: i32) -> Option<i32> {
        None
    }
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
        let (name, pid) = found.unwrap();
        assert_eq!(name, "claude");
        assert_ne!(pid, pgrp, "the agent, not its wrapper");
    }
}
