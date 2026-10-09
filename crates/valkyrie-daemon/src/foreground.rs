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
        // Background jobs a wrapper started are in other groups. Windows has no
        // groups: `pgrp` is the session's program, and all below it count.
        queue.extend(
            children(pid)
                .into_iter()
                .filter(|&child| cfg!(windows) || group_of(child) == Some(pgrp)),
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
        "node" | "bun" | "deno" => {
            let script = argv.get(1)?;
            // npm's Windows shim runs the package's `cli.js`, not a `claude` link.
            let package = Path::new(script).parent().and_then(|p| p.file_name());
            if stem(script).as_deref() == Some("cli") && package.is_some_and(|p| p == "claude-code")
            {
                return known("claude");
            }
            known(&stem(script)?)
        }
        _ => None,
    }
}

use sys::{children, cmdline_n, group_of, parent};
pub use sys::{cwd, started, tty};

/// The first two arguments: enough to tell the program and a runtime's script.
fn cmdline(pid: i32) -> Option<Vec<String>> {
    cmdline_n(pid, 2)
}

/// The agent `pid` runs, by its command line (see `agent_of`).
pub fn agent_running(pid: i32) -> Option<&'static str> {
    cmdline(pid).and_then(|argv| agent_of(&argv))
}

/// Whether `pid` runs `valk web`, the phone app's bridge (`valk [--socket S] web …`).
pub fn is_web_bridge(pid: i32) -> bool {
    cmdline_n(pid, 8).is_some_and(|argv| bridge_argv(&argv))
}

fn bridge_argv(argv: &[String]) -> bool {
    let Some((program, args)) = argv.split_first() else {
        return false;
    };
    if valkyrie_agents::program_name(program) != Some("valk") {
        return false;
    }
    let mut args = args.iter();
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--socket" => {
                args.next();
            }
            a if a.starts_with("--socket=") => {}
            "web" => {
                // `valk web pair` and the like print and exit; only the server bridges.
                return args.next().is_none_or(|a| a.starts_with('-'));
            }
            _ => return false,
        }
    }
    false
}

/// A terminal device as (major, minor), comparable across how each OS encodes it.
#[cfg_attr(windows, allow(dead_code))]
pub fn dev_key(major: u32, minor: u32) -> u64 {
    (u64::from(major) << 32) | u64::from(minor)
}

/// `pid` and the processes above it, nearest first, up to `MAX_PROCS`.
pub fn ancestry(pid: i32) -> Vec<i32> {
    let mut chain = vec![pid];
    while chain.len() < MAX_PROCS
        && let Some(up) = parent(*chain.last().unwrap())
        && up > 1
        && !chain.contains(&up)
    {
        chain.push(up);
    }
    chain
}

/// Linux: everything is in `/proc`.
#[cfg(target_os = "linux")]
mod sys {
    use std::path::PathBuf;

    /// A process's working directory.
    pub fn cwd(pid: i32) -> Option<PathBuf> {
        std::fs::read_link(format!("/proc/{pid}/cwd")).ok()
    }

    /// The first `n` arguments.
    pub fn cmdline_n(pid: i32, n: usize) -> Option<Vec<String>> {
        let raw = std::fs::read(format!("/proc/{pid}/cmdline")).ok()?;
        Some(
            raw.split(|&b| b == 0)
                .filter(|a| !a.is_empty())
                .take(n)
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

    /// Field 4 of `/proc/<pid>/stat`.
    pub fn parent(pid: i32) -> Option<i32> {
        stat_field(pid, 4)
    }

    /// The controlling terminal (field 7, `tty_nr`), as `dev_key`; `None` without one.
    pub fn tty(pid: i32) -> Option<u64> {
        let nr: u64 = stat_field(pid, 7)?;
        let major = (nr >> 8) & 0xfff;
        let minor = (nr & 0xff) | ((nr >> 12) & 0xfff00);
        (nr != 0).then(|| super::dev_key(major as u32, minor as u32))
    }

    /// When the process started (field 22, clock ticks since boot): with the pid,
    /// it names one process, even after the pid is reused.
    pub fn started(pid: i32) -> Option<u64> {
        stat_field(pid, 22)
    }

    /// Field `n` (from 1) of `/proc/<pid>/stat`, counted past the command name.
    fn stat_field<T: std::str::FromStr>(pid: i32, n: usize) -> Option<T> {
        let stat = std::fs::read_to_string(format!("/proc/{pid}/stat")).ok()?;
        let rest = &stat[stat.rfind(')')? + 1..];
        rest.split_whitespace().nth(n - 3)?.parse().ok()
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

    /// The first `n` arguments, from `KERN_PROCARGS2`: argc, the executable path,
    /// padding, then argv.
    pub fn cmdline_n(pid: i32, n: usize) -> Option<Vec<String>> {
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
                .take(n)
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

    fn bsdinfo(pid: i32) -> Option<libc::proc_bsdinfo> {
        // SAFETY: zeroed is a valid proc_bsdinfo; proc_pidinfo fills at most the
        // size given.
        let mut info: libc::proc_bsdinfo = unsafe { std::mem::zeroed() };
        let size = std::mem::size_of::<libc::proc_bsdinfo>() as libc::c_int;
        let n = unsafe {
            libc::proc_pidinfo(
                pid,
                libc::PROC_PIDTBSDINFO,
                0,
                (&mut info as *mut libc::proc_bsdinfo).cast(),
                size,
            )
        };
        (n == size).then_some(info)
    }

    pub fn parent(pid: i32) -> Option<i32> {
        bsdinfo(pid).map(|i| i.pbi_ppid as i32)
    }

    pub fn tty(pid: i32) -> Option<u64> {
        let dev = bsdinfo(pid)?.e_tdev as libc::dev_t;
        (dev != 0 && dev != libc::dev_t::MAX)
            .then(|| super::dev_key(libc::major(dev) as u32, libc::minor(dev) as u32))
    }

    pub fn started(pid: i32) -> Option<u64> {
        let info = bsdinfo(pid)?;
        Some(info.pbi_start_tvsec * 1_000_000 + info.pbi_start_tvusec)
    }
}

/// Windows: a Toolhelp snapshot for the process tree, and each process's own
/// memory (its PEB) for its command line and directory.
#[cfg(windows)]
mod sys {
    use std::path::PathBuf;
    use std::sync::{Arc, Mutex};
    use std::time::{Duration, Instant};
    use windows_sys::Wdk::System::Threading::{
        NtQueryInformationProcess, ProcessBasicInformation, ProcessCommandLineInformation,
    };
    use windows_sys::Win32::Foundation::{
        CloseHandle, FILETIME, HANDLE, HLOCAL, INVALID_HANDLE_VALUE, LocalFree, UNICODE_STRING,
    };
    use windows_sys::Win32::System::Diagnostics::Debug::ReadProcessMemory;
    use windows_sys::Win32::System::Diagnostics::ToolHelp::{
        CreateToolhelp32Snapshot, PROCESSENTRY32W, Process32FirstW, Process32NextW,
        TH32CS_SNAPPROCESS,
    };
    use windows_sys::Win32::System::Threading::{
        GetProcessTimes, OpenProcess, PROCESS_QUERY_INFORMATION, PROCESS_QUERY_LIMITED_INFORMATION,
        PROCESS_VM_READ,
    };
    use windows_sys::Win32::UI::Shell::CommandLineToArgvW;

    struct Proc {
        pid: u32,
        parent: u32,
        exe: String,
    }

    /// Every process, from a snapshot at most this old: one look at a session's
    /// tree takes several.
    const SNAPSHOT_FOR: Duration = Duration::from_millis(200);

    fn procs() -> Arc<Vec<Proc>> {
        static LAST: Mutex<Option<(Instant, Arc<Vec<Proc>>)>> = Mutex::new(None);
        let mut last = LAST.lock().unwrap();
        if let Some((at, procs)) = &*last
            && at.elapsed() < SNAPSHOT_FOR
        {
            return procs.clone();
        }
        let procs = Arc::new(snapshot());
        *last = Some((Instant::now(), procs.clone()));
        procs
    }

    fn snapshot() -> Vec<Proc> {
        let mut out = Vec::new();
        // SAFETY: a snapshot handle we close, and an entry sized as the API wants.
        unsafe {
            let snap = CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0);
            if snap == INVALID_HANDLE_VALUE {
                return out;
            }
            let mut entry: PROCESSENTRY32W = std::mem::zeroed();
            entry.dwSize = size_of::<PROCESSENTRY32W>() as u32;
            let mut ok = Process32FirstW(snap, &mut entry);
            while ok != 0 {
                let len = entry.szExeFile.iter().position(|&c| c == 0).unwrap_or(260);
                out.push(Proc {
                    pid: entry.th32ProcessID,
                    parent: entry.th32ParentProcessID,
                    exe: String::from_utf16_lossy(&entry.szExeFile[..len]),
                });
                ok = Process32NextW(snap, &mut entry);
            }
            CloseHandle(snap);
        }
        out
    }

    /// An open process handle, closed on drop.
    struct Process(HANDLE);

    impl Process {
        fn open(pid: i32, access: u32) -> Option<Process> {
            // SAFETY: plain call; a null handle means failure.
            let handle = unsafe { OpenProcess(access, 0, u32::try_from(pid).ok()?) };
            (!handle.is_null()).then_some(Process(handle))
        }

        /// A `T` at `address` in the process's memory.
        fn read<T: Copy>(&self, address: usize) -> Option<T> {
            let mut value = std::mem::MaybeUninit::<T>::uninit();
            let mut got = 0;
            // SAFETY: reads into a buffer of exactly size_of::<T>().
            let ok = unsafe {
                ReadProcessMemory(
                    self.0,
                    address as *const _,
                    value.as_mut_ptr().cast(),
                    size_of::<T>(),
                    &mut got,
                )
            };
            // SAFETY: fully written when the call read all of it.
            (ok != 0 && got == size_of::<T>()).then(|| unsafe { value.assume_init() })
        }
    }

    impl Drop for Process {
        fn drop(&mut self) {
            // SAFETY: we opened it.
            unsafe { CloseHandle(self.0) };
        }
    }

    /// `PROCESS_BASIC_INFORMATION`, whose windows-sys type needs another feature.
    #[repr(C)]
    struct BasicInfo {
        exit_status: i32,
        peb: usize,
        affinity: usize,
        priority: i32,
        pid: usize,
        parent: usize,
    }

    /// The process's current directory, from its PEB: `ProcessParameters` at 0x20,
    /// whose `CurrentDirectory.DosPath` is a UNICODE_STRING at 0x38 (64-bit layout,
    /// which this build and the processes it reads share).
    #[cfg(target_pointer_width = "64")]
    pub fn cwd(pid: i32) -> Option<PathBuf> {
        let process = Process::open(pid, PROCESS_QUERY_INFORMATION | PROCESS_VM_READ)?;
        // SAFETY: zeroed is a valid BasicInfo, which the call fills.
        let mut info: BasicInfo = unsafe { std::mem::zeroed() };
        let mut len = 0;
        // SAFETY: the buffer holds the size given.
        let status = unsafe {
            NtQueryInformationProcess(
                process.0,
                ProcessBasicInformation,
                (&raw mut info).cast(),
                size_of::<BasicInfo>() as u32,
                &mut len,
            )
        };
        if status != 0 || info.peb == 0 {
            return None;
        }
        let params: usize = process.read(info.peb + 0x20)?;
        let length: u16 = process.read(params + 0x38)?;
        let buffer: usize = process.read(params + 0x38 + 8)?;
        if length == 0 || !length.is_multiple_of(2) || buffer == 0 {
            return None;
        }
        let mut wide = vec![0u16; length as usize / 2];
        let mut got = 0;
        // SAFETY: reads into a buffer of exactly `length` bytes.
        let ok = unsafe {
            ReadProcessMemory(
                process.0,
                buffer as *const _,
                wide.as_mut_ptr().cast(),
                length as usize,
                &mut got,
            )
        };
        if ok == 0 || got != length as usize {
            return None;
        }
        let mut dir = String::from_utf16(&wide).ok()?;
        // `C:\repo\`, except a drive's root.
        if dir.len() > 3 && dir.ends_with('\\') {
            dir.pop();
        }
        Some(PathBuf::from(dir))
    }

    #[cfg(not(target_pointer_width = "64"))]
    pub fn cwd(_: i32) -> Option<PathBuf> {
        None
    }

    /// The first `n` arguments, split as the C runtime would.
    pub fn cmdline_n(pid: i32, n: usize) -> Option<Vec<String>> {
        let process = Process::open(pid, PROCESS_QUERY_LIMITED_INFORMATION)?;
        let mut buf = vec![0u64; 512];
        loop {
            let mut len = 0u32;
            // SAFETY: the buffer holds the size given; it starts with a
            // UNICODE_STRING pointing into itself.
            let status = unsafe {
                NtQueryInformationProcess(
                    process.0,
                    ProcessCommandLineInformation,
                    buf.as_mut_ptr().cast(),
                    (buf.len() * 8) as u32,
                    &mut len,
                )
            };
            if status == 0 {
                break;
            }
            // STATUS_INFO_LENGTH_MISMATCH: `len` says how much it needs.
            if (len as usize) <= buf.len() * 8 || len > 1 << 20 {
                return None;
            }
            buf = vec![0u64; (len as usize).div_ceil(8)];
        }
        // SAFETY: the call above filled a UNICODE_STRING whose buffer lies in `buf`.
        let line: Vec<u16> = unsafe {
            let s = &*(buf.as_ptr() as *const UNICODE_STRING);
            if s.Buffer.is_null() {
                return None;
            }
            std::slice::from_raw_parts(s.Buffer, s.Length as usize / 2).to_vec()
        };
        split_args(&line, n)
    }

    fn split_args(line: &[u16], n: usize) -> Option<Vec<String>> {
        let line: Vec<u16> = line.iter().copied().chain([0]).collect();
        let mut argc = 0;
        // SAFETY: a NUL-terminated string; the result is freed below.
        let argv = unsafe { CommandLineToArgvW(line.as_ptr(), &mut argc) };
        if argv.is_null() {
            return None;
        }
        let args = (0..argc.max(0) as usize)
            .take(n)
            .map(|i| {
                // SAFETY: CommandLineToArgvW gave `argc` NUL-terminated strings.
                unsafe {
                    let arg = *argv.add(i);
                    let len = (0..).take_while(|&j| *arg.add(j) != 0).count();
                    String::from_utf16_lossy(std::slice::from_raw_parts(arg, len))
                }
            })
            .collect();
        // SAFETY: allocated by CommandLineToArgvW.
        unsafe { LocalFree(argv as HLOCAL) };
        Some(args)
    }

    pub fn children(pid: i32) -> Vec<i32> {
        procs()
            .iter()
            .filter(|p| p.parent as i32 == pid && p.pid as i32 != pid)
            .map(|p| p.pid as i32)
            .collect()
    }

    /// No process groups on Windows (`agent` takes every descendant).
    pub fn group_of(_: i32) -> Option<i32> {
        None
    }

    /// The process that started `pid`, if it is still that one: Windows keeps a
    /// dead parent's pid, which a newer process may have since.
    pub fn parent(pid: i32) -> Option<i32> {
        let up = procs().iter().find(|p| p.pid as i32 == pid)?.parent as i32;
        (up > 0 && started(up)? <= started(pid)?).then_some(up)
    }

    /// Windows has no controlling terminal. Its nearest stand-in: `pid` runs under a
    /// terminal or the desktop, by an unbroken line of parents, so someone started
    /// it by hand. Sessions run under the daemon, which `caller` finds first.
    pub fn tty(pid: i32) -> Option<u64> {
        const HOSTS: &[&str] = &[
            "windowsterminal.exe",
            "openconsole.exe",
            "conhost.exe",
            "explorer.exe",
            "sshd.exe",
            "wezterm-gui.exe",
            "alacritty.exe",
            "mintty.exe",
            "conemu64.exe",
            "tabby.exe",
            "code.exe",
            "cursor.exe",
        ];
        let procs = procs();
        let exe = |pid: i32| {
            procs
                .iter()
                .find(|p| p.pid as i32 == pid)
                .map(|p| p.exe.to_ascii_lowercase())
        };
        super::ancestry(pid)
            .into_iter()
            .skip(1)
            .any(|up| exe(up).is_some_and(|e| HOSTS.contains(&e.as_str())))
            .then_some(0)
    }

    /// When the process started (100 ns since 1601): with the pid, it names one
    /// process, even after the pid is reused.
    pub fn started(pid: i32) -> Option<u64> {
        let process = Process::open(pid, PROCESS_QUERY_LIMITED_INFORMATION)?;
        let zero = FILETIME {
            dwLowDateTime: 0,
            dwHighDateTime: 0,
        };
        let (mut created, mut exited, mut kernel, mut user) = (zero, zero, zero, zero);
        // SAFETY: valid out pointers.
        let ok = unsafe {
            GetProcessTimes(process.0, &mut created, &mut exited, &mut kernel, &mut user)
        };
        (ok != 0)
            .then(|| (u64::from(created.dwHighDateTime) << 32) | u64::from(created.dwLowDateTime))
    }

    #[cfg(test)]
    mod tests {
        #[test]
        fn splits_a_command_line_as_the_runtime_does() {
            let line: Vec<u16> = r#""C:\Program Files\node.exe" C:\x\codex.js --yolo"#
                .encode_utf16()
                .collect();
            assert_eq!(
                super::split_args(&line, 2).unwrap(),
                [r"C:\Program Files\node.exe", r"C:\x\codex.js"]
            );
        }
    }
}

/// Elsewhere: no agent detection; sessions keep their spawn directory.
#[cfg(not(any(target_os = "linux", target_os = "macos", windows)))]
mod sys {
    pub fn cwd(_: i32) -> Option<std::path::PathBuf> {
        None
    }
    pub fn cmdline_n(_: i32, _: usize) -> Option<Vec<String>> {
        None
    }
    pub fn children(_: i32) -> Vec<i32> {
        Vec::new()
    }
    pub fn group_of(_: i32) -> Option<i32> {
        None
    }
    pub fn parent(_: i32) -> Option<i32> {
        None
    }
    pub fn tty(_: i32) -> Option<u64> {
        None
    }
    pub fn started(_: i32) -> Option<u64> {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(any(target_os = "linux", target_os = "macos"))]
    #[test]
    fn ancestry_starts_here_and_climbs_to_the_parent() {
        let me = std::process::id() as i32;
        let chain = ancestry(me);
        assert_eq!(chain[0], me);
        // SAFETY: getppid cannot fail.
        let parent = unsafe { libc::getppid() };
        if parent > 1 {
            assert_eq!(chain.get(1), Some(&parent));
        }
        // The same process reads the same start time twice.
        assert!(started(me).is_some());
        assert_eq!(started(me), started(me));
    }

    #[test]
    fn knows_the_web_bridge_by_its_arguments() {
        let yes = |a: &[&str]| bridge_argv(&argv(a));
        assert!(yes(&["/home/u/.local/bin/valk", "web"]));
        assert!(yes(&[
            "valk",
            "--socket",
            "/s.sock",
            "web",
            "--listen",
            "127.0.0.1:1"
        ]));
        assert!(yes(&["valk", "--socket=/s", "web"]));
        assert!(!yes(&["valk", "web", "pair"]));
        assert!(!yes(&["valk", "new", "--", "web"]));
        assert!(!yes(&["sh", "web"]));
        assert!(!yes(&["valk"]));
    }

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
        assert_eq!(
            agent_of(&argv(&[
                "node",
                "/usr/lib/node_modules/@anthropic-ai/claude-code/cli.js"
            ])),
            Some("claude")
        );
        assert_eq!(agent_of(&argv(&["node", "/srv/app/cli.js"])), None);
        assert_eq!(agent_of(&argv(&["vim", "claude.md"])), None);
        assert_eq!(agent_of(&argv(&["fish"])), None);
        assert_eq!(agent_of(&[]), None);
    }

    /// A wrapper script in the foreground, the agent its child: found below it.
    #[cfg(unix)]
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
