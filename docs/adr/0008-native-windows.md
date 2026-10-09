# ADR-0008: Native Windows support

## Decision

Windows x64 uses the same protocol and terminal engine as Linux and macOS, with
platform adapters for IPC, PTYs, process inspection, and console input.

- `valkyrie-proto::ipc` uses Tokio named pipes on Windows and Unix sockets elsewhere.
  Pipe names hash the socket path, keeping `--socket` and `VALK_SOCKET` usable.
  Pipes reject remote clients, grant access to the current user's SID, and verify
  the server's user before sending a request. The first instance prevents two
  daemons from owning the same endpoint. Hook exchanges have bounded waiting time.
- `valkyrie-daemon::pty` uses portable-pty's ConPTY master on Windows. A job object
  terminates the session's program tree when killed or when the daemon exits.
  The exit waiter closes the ConPTY master outside the screen lock, allowing the
  reader to drain and exit rather than retaining its session indefinitely.
- Standard npm `.cmd` launchers for Claude and Codex resolve to their Node entry
  points, so arguments and Claude's hook settings avoid cmd.exe interpretation.
  Other batch programs must run inside a shell tab.
- Windows process inspection uses Toolhelp snapshots, process creation times,
  command-line queries, and the 64-bit PEB's current-directory field. Windows has
  no foreground process group, so agent detection searches session descendants.
- The TUI enables virtual terminal input/output and polls for console resizing.
  New shell sessions prefer PowerShell 7, then Windows PowerShell. Links use the
  Windows browser opener and sounds use the Windows audio API.
- State lives in `%LOCALAPPDATA%\Valkyrie`; configuration lives in
  `%APPDATA%\Valkyrie`. XDG overrides remain available.
- `valk setup web` installs a logon task in Task Scheduler, retaining the selected
  shell environment and writing output to the web log.

Windows does not offer Unix's exec-based live handoff. An upgrade saves the restore
list, starts the successor daemon, and exits. Restorable agent conversations resume;
shells return in their directories; arbitrary running commands do not resume.

`install.ps1` downloads a checksummed Windows ZIP and installs into a new version
directory, then updates the `valk.cmd` launcher. Existing executables can keep running
while a new one is installed. The WSL installer remains as `install-wsl.ps1`.

## Validation and limits

The Linux workspace tests pass with local socket and PTY access. Windows Rust code
and tests pass a cross-target type check and Clippy from Linux, using clang-cl and
stub CRT headers for ring. This does not produce or run a linked Windows executable.
The Windows CI job builds with MSVC. Named-pipe and ConPTY integration tests have
passed on Windows, covering shell directories, exit codes, termination, duplicate
daemons, and npm launcher arguments. CI also parses both PowerShell installers.
Release publication requires both platform jobs to pass. Interactive Windows
Terminal input and Task Scheduler setup still need a manual Windows check.

Windows ARM64 native builds and 32-bit process directory inspection are outside this
implementation. Windows ARM64 can continue using the WSL installer. Job assignment
occurs after portable-pty starts the program, so descendants created before that
assignment may not join the job. Terminal behavior, daemon restart recovery,
Task Scheduler, and interactive installer execution need validation on Windows.
