# ADR-0006: Upgrade without losing sessions (re-exec handoff)

Status: accepted
Date: 2026-10-07

## Context
The daemon owns every agent PTY (ADR-0002), so restarting it to pick up a new build kills every session. During dogfooding that happens on every fix. herdr solves the same problem with `herdr update --handoff`. Requirement: `overseer upgrade` switches the running daemon to the binary on disk, and every session (process, screen, agent state, event log) carries on.

## Options
1. **Hand fds to a new, separate daemon over the socket (SCM_RIGHTS).** The new daemon is not the agents' parent: it can never `waitpid` them, so exit codes and reaping are lost, and the agents end up reparented to init.
2. **The daemon re-execs itself as the new binary.** `execve` keeps the pid, so agents stay its children and `waitpid` keeps working. File descriptors without `CLOEXEC` survive. This is the classic hot-reload trick (nginx, haproxy).

## Decision
Option 2.

1. `overseer upgrade` sends `Upgrade { exe }` to the daemon. `exe` defaults to the calling binary, which is the freshly built one.
2. **Preflight, before anything is frozen:** the daemon pipes a handoff of the live sessions into `<exe> daemon --handoff-check`. The new binary must parse it and print our `HANDOFF_VERSION`, or the upgrade is refused and nothing changes.
3. **Freeze:**
   1. The accept loop stops accepting. New connections, including hooks, wait in the listen backlog. Connections already accepted get 50 ms to have their frames handled first; a hook writes its one frame right after connecting.
   2. A lock shared with `spawn` is taken exclusively, so no child is forked that the snapshot would miss. Spawns during the handoff are refused.
   3. Sessions killed but not yet reaped get their SIGKILL now, since the grace timer would die with the old image. This cuts short the 2 s SIGHUP grace of a session killed just before an upgrade. Any still unreaped after 500 ms are passed on for the new image to reap.
   4. Readers and writers stop. Both `poll` their PTY together with a stop pipe and only act when it isn't set. No output byte is ever read and then dropped; unread output stays in the kernel. A writer stopped mid-buffer puts the unwritten rest back at the front of its queue, and the whole queue goes into the handoff. Input is never dropped, however long the program has not been reading.
4. **Hand over:**
   1. Session metadata goes into a `memfd`: id, name, command, cwd, pid, transcript and event-log paths, offset, size, agent status, the exit code if already exited, and unwritten input. The daemon's `boot` identity goes too.
   2. `CLOEXEC` is cleared on the memfd, the listener and each PTY master.
   3. The daemon calls `execve(exe, [.., "daemon", "--resume-fd", N])`. It sends no reply on success: the requesting client sees its connection close, then confirms the switch through a new connection's `Hello`.
5. **Resume:**
   1. The new process reads the memfd and adopts the listener and masters.
   2. Before any thread starts, `CLOEXEC` goes back on every inherited fd.
   3. It rebuilds each screen by replaying the transcript up to the recorded offset, applying the logged resizes. Terminal query replies are discarded during replay.
   4. It reopens logs for append, restarts a tracker from the saved status, re-queues unwritten input, and waits on each pid with `waitpid`.
   5. Adoption is best effort. A missing transcript gives a blank screen, and a missing log is recreated. If a session still can't be adopted, its program is reaped rather than left a zombie.
   6. Its `Hello` reports `generation + 1` and the same `boot`, so the client can confirm the switch happened. A daemon that restarted instead has a different `boot`.
6. **Failure:** if `execve` itself fails, the daemon puts `CLOEXEC` back, unfreezes (restarts readers, writers and accepting) and replies with the error.
7. **Clients:** connected clients get EOF at exec. The TUI reconnects automatically and re-attaches to the session it was showing, but only when `boot` matches; after a real restart, ids start over.

## Consequences
- An upgrade costs one transcript replay per session and a brief pause in output, never lost output or input. Replay time grows with transcript size; snapshotting screens or replaying from a recorded clear-screen offset would bound it (follow-up).
- A client accepted before the freeze whose frame isn't handled within 50 ms loses that frame. A hook writes its frame right after connecting, so this needs an unusually slow hook.
- Between the preflight and the exec, the binary at `exe` could be replaced (for example, a `cargo build` finishing at that moment). If the replacement can't resume, the sessions are lost. The window is milliseconds.
- A child that exits in the instant between the freeze and the exec is reaped by the old image's waiter thread; the new image then gets `ECHILD` and records the exit with an unknown code.
- The handoff format is versioned separately from the wire protocol. Bumping `HANDOFF_VERSION` means an upgrade across that change is refused instead of corrupting sessions; that build needs a normal restart.
- The daemon from before this ADR cannot hand off; switching to the first build that has it needs one plain restart.
