//! Per-session state machine (DESIGN §14.3). Deterministic: time comes in as `now`
//! (ms since the epoch), so recorded sessions replay to the same states.

use crate::{AgentEvent, Screen};
use valkyrie_proto::{AgentState, AgentStatus, AskKind};

/// Screen idle while hooks say busy for this long → `Interrupted` (Leader's
/// `stale_after`; no hook fires on Esc-interrupt or Esc-deny).
pub const INTERRUPT_AFTER_MS: u64 = 10_000;
/// Shorter grace when a permission dialog was on screen and gave way to the idle
/// prompt: that is an Esc-deny (an approval shows the tool running instead).
pub const DENY_AFTER_MS: u64 = 2_000;
/// A hook this far behind the newest one means the clock stepped, not a race.
const CLOCK_STEP_US: u64 = 5_000_000;
/// Working with no output or events for this long → `Stale`.
pub const STALE_AFTER_MS: u64 = 5 * 60_000;

pub struct Tracker {
    status: AgentStatus,
    /// Send time of the newest hook applied; older ones are dropped.
    last_hook_us: u64,
    last_activity_ms: u64,
    /// Latest PTY output and latest screen scan: an idle verdict only stands while
    /// nothing has been drawn since it was taken (a spinner never leaves the 150 ms
    /// quiet gap a rescan needs).
    last_output_ms: u64,
    last_scan_ms: u64,
    /// The latest screen verdict and since when it has said `Idle`.
    screen: Option<Screen>,
    screen_idle_since: Option<u64>,
    /// The permission dialog has been on screen during the current `NeedsInput`.
    dialog_seen: bool,
    /// A permission request an automatic reviewer took over: its summary, so the
    /// request can go back to the human if the reviewer hands it on with a dialog.
    reviewing: Option<String>,
    /// Whether the agent can miss transitions that only the screen shows.
    hook_gaps: bool,
    ended: bool,
}

impl Tracker {
    pub fn new(agent: &str, hook_gaps: bool, now: u64) -> Self {
        Self {
            status: AgentStatus::new(agent, now),
            last_hook_us: 0,
            last_activity_ms: now,
            last_output_ms: 0,
            last_scan_ms: 0,
            screen: None,
            screen_idle_since: None,
            dialog_seen: false,
            reviewing: None,
            hook_gaps,
            ended: false,
        }
    }

    /// Continues from a status carried across a daemon upgrade (ADR-0006). Timers and
    /// screen verdicts start fresh; `ended` if the program had already exited.
    pub fn resume(status: AgentStatus, hook_gaps: bool, ended: bool, now: u64) -> Self {
        let mut tracker = Self::new(&status.agent, hook_gaps, now);
        tracker.status = status;
        tracker.ended = ended;
        tracker
    }

    pub fn status(&self) -> &AgentStatus {
        &self.status
    }

    /// Applies hook events sent at `sent_us`. Returns whether the status changed.
    /// `watching`: a client is attached, so whatever happens now is already seen.
    pub fn hook(&mut self, events: &[AgentEvent], sent_us: u64, now: u64, watching: bool) -> bool {
        // Drop hooks that lost a race, but not every hook after the clock stepped back.
        let behind = self.last_hook_us.saturating_sub(sent_us);
        if self.ended || (behind > 0 && behind < CLOCK_STEP_US) {
            return false;
        }
        self.last_hook_us = sent_us;
        let mut changed = !self.status.hooked;
        self.status.hooked = true;
        for event in events {
            changed |= self.apply(event, now, watching);
        }
        changed
    }

    /// Applies one event from any source.
    pub fn apply(&mut self, event: &AgentEvent, now: u64, watching: bool) -> bool {
        use AgentEvent as E;
        use AgentState as S;
        if self.ended {
            return false;
        }
        self.last_activity_ms = now;
        // A hand-over to the human comes after the tool's PreToolUse; anything else
        // settles the reviewed request.
        if !matches!(event, E::ToolStarted) {
            self.reviewing = None;
        }
        let state = self.status.state;
        match event {
            E::SessionStarted | E::SessionEnded | E::Interrupted => {
                self.set(S::Idle, None, None, now, watching)
            }
            E::PromptSubmitted | E::ToolStarted | E::ToolFinished => {
                self.set(S::Working, None, None, now, watching)
            }
            E::PermissionAsked { summary } => self.set(
                S::NeedsInput,
                Some(AskKind::Permission),
                Some(summary.clone()),
                now,
                watching,
            ),
            E::QuestionAsked { summary } => self.set(
                S::NeedsInput,
                Some(AskKind::Question),
                Some(summary.clone()),
                now,
                watching,
            ),
            E::InputAsked { summary } => self.set(
                S::NeedsInput,
                Some(AskKind::Input),
                Some(summary.clone()),
                now,
                watching,
            ),
            // Trails PermissionAsked by ~6 s with vaguer text, sometimes after the
            // dialog was already answered or dismissed. It only counts when
            // PermissionAsked was missed, i.e. the session still looks busy.
            E::PermissionNotified { summary } => {
                if state != S::Working {
                    false
                } else {
                    self.set(
                        S::NeedsInput,
                        Some(AskKind::Permission),
                        Some(summary.clone()),
                        now,
                        watching,
                    )
                }
            }
            E::TurnEnded { summary } => {
                self.set(S::ReviewReady, None, summary.clone(), now, watching)
            }
            E::TurnFailed { summary } => {
                self.set(S::Blocked, None, Some(summary.clone()), now, watching)
            }
            // Agents with hooks ring the bell for things their hooks already report.
            E::Bell => {
                if self.status.hooked || state == S::NeedsInput {
                    false
                } else {
                    self.set(
                        S::NeedsInput,
                        Some(AskKind::Bell),
                        Some("bell".into()),
                        now,
                        watching,
                    )
                }
            }
            E::Exited { code } => {
                let changed = match code {
                    Some(0) | None => self.set(S::Exited, None, None, now, watching),
                    Some(code) => self.set(
                        S::Blocked,
                        None,
                        Some(format!("exited with code {code}")),
                        now,
                        watching,
                    ),
                };
                self.ended = true;
                changed
            }
        }
    }

    /// PTY output arrived.
    pub fn output(&mut self, now: u64) -> bool {
        self.last_activity_ms = now;
        self.last_output_ms = now;
        if self.status.state == AgentState::Stale && !self.ended {
            return self.set(AgentState::Working, None, None, now, false);
        }
        false
    }

    /// A fresh screen verdict (taken once output has gone quiet).
    pub fn screen(&mut self, verdict: Option<Screen>, now: u64, watching: bool) -> bool {
        use AgentState as S;
        if self.screen_idle_since.is_none() || verdict != Some(Screen::Idle) {
            self.screen_idle_since = (verdict == Some(Screen::Idle)).then_some(now);
        }
        self.screen = verdict.clone();
        self.last_scan_ms = now;
        if matches!(verdict, Some(Screen::Prompt { .. }))
            && self.status.ask == Some(AskKind::Permission)
        {
            self.dialog_seen = true;
        }
        if self.ended {
            return false;
        }
        if self.status.hooked {
            return self.review(verdict, now, watching) | self.check_interrupted(now, watching);
        }
        // Heuristics only: before the first hook (trust prompts), or no hooks at all.
        let state = self.status.state;
        let screen_ask = self.status.ask == Some(AskKind::Screen);
        match verdict {
            Some(Screen::Prompt { summary }) => self.set(
                S::NeedsInput,
                Some(AskKind::Screen),
                Some(summary),
                now,
                watching,
            ),
            Some(Screen::Busy | Screen::Reviewing) => {
                self.set(S::Working, None, None, now, watching)
            }
            Some(Screen::Idle) if state == S::Working => {
                self.set(S::ReviewReady, None, None, now, watching)
            }
            Some(Screen::Idle) | None if screen_ask => self.set(S::Idle, None, None, now, watching),
            _ => false,
        }
    }

    /// A screen verdict taken while output is still coming. Only the auto-reviewer
    /// checks use it: a busy screen can look idle mid-frame (Claude draws its prompt
    /// box under the spinner), so idle detection and heuristics wait for quiet.
    pub fn glance(&mut self, verdict: Option<Screen>, now: u64, watching: bool) -> bool {
        !self.ended && self.status.hooked && self.review(verdict, now, watching)
    }

    /// Codex fires `PermissionRequest` before its automatic reviewer decides, so the
    /// request only needs the human once the reviewer hands it over with a dialog.
    fn review(&mut self, verdict: Option<Screen>, now: u64, watching: bool) -> bool {
        use AgentState as S;
        let s = &self.status;
        match verdict {
            Some(Screen::Reviewing)
                if s.state == S::NeedsInput && s.ask == Some(AskKind::Permission) =>
            {
                let summary = s.summary.clone();
                let changed = self.set(
                    S::Working,
                    None,
                    summary.as_ref().map(|s| format!("auto-review: {s}")),
                    now,
                    watching,
                );
                self.reviewing = summary;
                changed
            }
            Some(Screen::Prompt { .. }) if s.state == S::Working && self.reviewing.is_some() => {
                let summary = self.reviewing.take();
                let changed = self.set(
                    S::NeedsInput,
                    Some(AskKind::Permission),
                    summary,
                    now,
                    watching,
                );
                // Static from here on, so it won't be rescanned.
                self.dialog_seen = true;
                changed
            }
            _ => false,
        }
    }

    /// Timers: stale, and the interrupted check while the screen stays idle.
    pub fn tick(&mut self, now: u64, watching: bool) -> bool {
        if self.ended {
            return false;
        }
        if self.status.state == AgentState::Working
            && now.saturating_sub(self.last_activity_ms) >= STALE_AFTER_MS
        {
            return self.set(AgentState::Stale, None, None, now, watching);
        }
        self.status.hooked && self.check_interrupted(now, watching)
    }

    fn check_interrupted(&mut self, now: u64, watching: bool) -> bool {
        let busy = match self.status.state {
            AgentState::Working => true,
            AgentState::NeedsInput => self.status.ask == Some(AskKind::Permission),
            _ => false,
        };
        // Idle and silent within the current busy state: an idle screen left over
        // from before (e.g. the prompt was just submitted) doesn't count, and neither
        // does one that is still being redrawn (a busy screen can scan as idle).
        let idle_for = self
            .screen_idle_since
            .filter(|_| self.last_scan_ms >= self.last_output_ms)
            .map(|t| now.saturating_sub(t.max(self.status.since_ms).max(self.last_output_ms)));
        let grace = if self.dialog_seen {
            DENY_AFTER_MS
        } else {
            INTERRUPT_AFTER_MS
        };
        if self.hook_gaps && busy && idle_for.is_some_and(|d| d >= grace) {
            return self.set(
                AgentState::Interrupted,
                None,
                Some("no hook fired; the screen shows an idle prompt".into()),
                now,
                watching,
            );
        }
        false
    }

    /// The human looked at the session (attached to it).
    pub fn seen(&mut self) -> bool {
        let seq = self.status.seq;
        self.mark_seen(seq)
    }

    /// Marks transition `seq` seen; a newer transition is left alone.
    pub fn mark_seen(&mut self, seq: u64) -> bool {
        if self.status.seq != seq || self.status.seen {
            return false;
        }
        // A bell has no answer to wait for: looking at it settles it, and the next
        // bell queues again.
        if self.status.ask == Some(AskKind::Bell) {
            return self.set(AgentState::Idle, None, None, self.status.since_ms, true);
        }
        self.status.seen = true;
        true
    }

    /// Appends detail to the current summary (e.g. a diff stat computed later), if
    /// the session is still on transition `seq`.
    pub fn annotate(&mut self, seq: u64, extra: &str) -> bool {
        if self.status.seq != seq {
            return false;
        }
        self.status.summary = Some(match self.status.summary.take() {
            Some(s) => format!("{s} · {extra}"),
            None => extra.to_owned(),
        });
        true
    }

    fn set(
        &mut self,
        state: AgentState,
        ask: Option<AskKind>,
        summary: Option<String>,
        now: u64,
        watching: bool,
    ) -> bool {
        let s = &mut self.status;
        if s.state == state && s.ask == ask && s.summary == summary {
            return false;
        }
        if s.state != state || s.ask != ask {
            s.since_ms = now;
        }
        s.state = state;
        s.ask = ask;
        s.summary = summary;
        s.seq += 1;
        s.seen = watching;
        self.dialog_seen = false;
        if state != AgentState::Working {
            self.reviewing = None;
        }
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use AgentEvent as E;
    use AgentState as S;

    fn tracker() -> Tracker {
        Tracker::new("claude", true, 0)
    }

    fn hook(t: &mut Tracker, e: E, now: u64) -> bool {
        t.hook(&[e], now * 1000, now, false)
    }

    #[test]
    fn a_turn_goes_working_then_review_ready_until_seen() {
        let mut t = tracker();
        hook(&mut t, E::PromptSubmitted, 1);
        assert_eq!(t.status().state, S::Working);
        hook(
            &mut t,
            E::TurnEnded {
                summary: Some("Done.".into()),
            },
            2,
        );
        assert_eq!(t.status().state, S::ReviewReady);
        assert!(!t.status().seen);
        assert!(t.mark_seen(t.status().seq));
        assert!(t.status().seen);
        assert!(!t.mark_seen(t.status().seq), "already seen");
    }

    #[test]
    fn mark_seen_for_an_older_transition_is_ignored() {
        let mut t = tracker();
        hook(&mut t, E::TurnEnded { summary: None }, 1);
        let old = t.status().seq;
        hook(&mut t, E::PromptSubmitted, 2);
        hook(&mut t, E::TurnEnded { summary: None }, 3);
        assert!(!t.mark_seen(old));
        assert!(!t.status().seen);
    }

    #[test]
    fn finishing_while_watched_is_already_seen() {
        let mut t = tracker();
        t.hook(&[E::TurnEnded { summary: None }], 1, 1, true);
        assert!(t.status().seen);
    }

    #[test]
    fn approval_is_only_visible_through_tool_finished() {
        let mut t = tracker();
        hook(&mut t, E::PromptSubmitted, 1);
        hook(
            &mut t,
            E::PermissionAsked {
                summary: "Permission: Bash ls".into(),
            },
            2,
        );
        assert_eq!(t.status().state, S::NeedsInput);
        assert_eq!(t.status().ask, Some(AskKind::Permission));
        hook(&mut t, E::ToolFinished, 3);
        assert_eq!(t.status().state, S::Working);
    }

    #[test]
    fn late_permission_notification_keeps_the_specific_summary() {
        let mut t = tracker();
        hook(
            &mut t,
            E::PermissionAsked {
                summary: "Permission: Bash touch x".into(),
            },
            1,
        );
        let seq = t.status().seq;
        assert!(!hook(
            &mut t,
            E::PermissionNotified {
                summary: "Claude needs your permission".into()
            },
            7
        ));
        assert_eq!(
            t.status().summary.as_deref(),
            Some("Permission: Bash touch x")
        );
        assert_eq!(t.status().seq, seq);
    }

    #[test]
    fn out_of_order_hooks_are_dropped() {
        let mut t = tracker();
        t.hook(&[E::TurnEnded { summary: None }], 200, 2, false);
        assert!(!t.hook(&[E::PromptSubmitted], 100, 3, false));
        assert_eq!(t.status().state, S::ReviewReady);
    }

    #[test]
    fn idle_screen_while_working_becomes_interrupted_after_the_grace_period() {
        let mut t = tracker();
        hook(&mut t, E::PromptSubmitted, 0);
        t.screen(Some(Screen::Idle), 1_000, false);
        assert!(!t.tick(1_000 + INTERRUPT_AFTER_MS - 1, false));
        // A repeated idle verdict must not restart the clock.
        t.screen(Some(Screen::Idle), 5_000, false);
        assert!(t.tick(1_000 + INTERRUPT_AFTER_MS, false));
        assert_eq!(t.status().state, S::Interrupted);
    }

    #[test]
    fn an_idle_screen_from_before_the_busy_state_does_not_count() {
        // Live: interrupted, screen idle; then a new prompt must not be flagged at once.
        let mut t = tracker();
        t.screen(Some(Screen::Idle), 0, false);
        hook(&mut t, E::PromptSubmitted, 20_000);
        assert!(!t.tick(20_100, false));
        assert_eq!(t.status().state, S::Working);
        assert!(t.tick(20_000 + INTERRUPT_AFTER_MS, false));
    }

    #[test]
    fn late_permission_notification_is_ignored_once_settled() {
        let mut t = tracker();
        hook(&mut t, E::TurnEnded { summary: None }, 1);
        assert!(!hook(
            &mut t,
            E::PermissionNotified {
                summary: "Claude needs your permission".into()
            },
            2
        ));
        // But it still catches a permission whose PermissionRequest was missed.
        hook(&mut t, E::PromptSubmitted, 3);
        assert!(hook(
            &mut t,
            E::PermissionNotified {
                summary: "Claude needs your permission".into()
            },
            4
        ));
        assert_eq!(t.status().state, S::NeedsInput);
    }

    #[test]
    fn a_permission_dialog_giving_way_to_the_idle_prompt_is_a_quick_deny() {
        let mut t = tracker();
        hook(&mut t, E::PromptSubmitted, 0);
        hook(
            &mut t,
            E::PermissionAsked {
                summary: "Permission: Bash ls".into(),
            },
            1_000,
        );
        let dialog = Screen::Prompt {
            summary: "Do you want to proceed?".into(),
        };
        t.screen(Some(dialog), 1_200, false);
        t.screen(Some(Screen::Idle), 5_000, false);
        assert!(!t.tick(5_000 + DENY_AFTER_MS - 1, false));
        assert!(t.tick(5_000 + DENY_AFTER_MS, false));
        assert_eq!(t.status().state, S::Interrupted);
        // Without the dialog having been seen, the long grace applies.
        let mut t = tracker();
        hook(
            &mut t,
            E::PermissionAsked {
                summary: "Permission: Bash ls".into(),
            },
            1_000,
        );
        t.screen(Some(Screen::Idle), 5_000, false);
        assert!(!t.tick(5_000 + DENY_AFTER_MS, false));
    }

    #[test]
    fn output_after_an_idle_scan_voids_it() {
        // Review #2: the spinner keeps drawing, so no rescan replaces the early idle.
        let mut t = tracker();
        hook(&mut t, E::PromptSubmitted, 0);
        t.screen(Some(Screen::Idle), 400, false);
        for ms in (500..30_000).step_by(100) {
            t.output(ms);
            assert!(!t.tick(ms, false), "flagged at {ms}");
        }
        assert_eq!(t.status().state, S::Working);
    }

    #[test]
    fn timer_transitions_while_watched_are_seen() {
        let mut t = tracker();
        hook(&mut t, E::PromptSubmitted, 0);
        assert!(t.tick(STALE_AFTER_MS, true));
        assert!(t.status().seen);
    }

    #[test]
    fn a_seen_bell_settles_and_the_next_bell_queues_again() {
        let mut t = Tracker::new("generic", false, 0);
        t.apply(&E::Bell, 1, false);
        assert!(t.seen());
        assert_eq!(t.status().state, S::Idle);
        assert!(t.apply(&E::Bell, 2, false));
        assert_eq!(t.status().state, S::NeedsInput);
        assert!(!t.status().seen);
    }

    #[test]
    fn a_clock_step_back_does_not_freeze_hooks() {
        let mut t = tracker();
        t.hook(&[E::PromptSubmitted], 100_000_000, 1, false);
        assert!(!t.hook(&[E::TurnEnded { summary: None }], 99_000_000, 2, false));
        assert!(t.hook(&[E::TurnEnded { summary: None }], 10_000_000, 3, false));
        assert_eq!(t.status().state, S::ReviewReady);
    }

    #[test]
    fn busy_screen_resets_the_interrupt_clock() {
        let mut t = tracker();
        hook(&mut t, E::PromptSubmitted, 0);
        t.screen(Some(Screen::Idle), 1_000, false);
        t.screen(Some(Screen::Busy), 2_000, false);
        t.screen(Some(Screen::Idle), 3_000, false);
        assert!(!t.tick(1_000 + INTERRUPT_AFTER_MS, false));
        assert!(t.tick(3_000 + INTERRUPT_AFTER_MS, false));
    }

    #[test]
    fn agents_with_an_interrupt_hook_skip_the_screen_check() {
        let mut t = Tracker::new("codex", false, 0);
        hook(&mut t, E::PromptSubmitted, 0);
        t.screen(Some(Screen::Idle), 0, false);
        assert!(!t.tick(INTERRUPT_AFTER_MS * 2, false));
        assert_eq!(t.status().state, S::Working);
    }

    #[test]
    fn an_auto_reviewed_permission_only_needs_the_human_once_handed_over() {
        let mut t = Tracker::new("codex", false, 0);
        hook(&mut t, E::PromptSubmitted, 0);
        let ask = E::PermissionAsked {
            summary: "Permission: Bash touch a".into(),
        };
        hook(&mut t, ask.clone(), 1);
        assert_eq!(t.status().state, S::NeedsInput);
        assert!(t.screen(Some(Screen::Reviewing), 2, false));
        assert_eq!(t.status().state, S::Working);
        assert_eq!(
            t.status().summary.as_deref(),
            Some("auto-review: Permission: Bash touch a")
        );
        // Reviewer approves: the tool runs and finishes.
        hook(&mut t, E::ToolFinished, 3);
        let dialog = || {
            Some(Screen::Prompt {
                summary: "Would you like to run…?".into(),
            })
        };
        assert!(!t.screen(dialog(), 4, false));
        assert_eq!(t.status().state, S::Working);
        // Reviewer hands the next one to the human.
        hook(&mut t, ask, 5);
        t.screen(Some(Screen::Reviewing), 6, false);
        assert!(t.screen(dialog(), 7, false));
        assert_eq!(t.status().state, S::NeedsInput);
        assert_eq!(t.status().ask, Some(AskKind::Permission));
        assert_eq!(
            t.status().summary.as_deref(),
            Some("Permission: Bash touch a")
        );
    }

    #[test]
    fn idle_looking_glances_under_a_spinner_never_interrupt() {
        // Claude draws its prompt box under the spinner, so a mid-output frame scans
        // as idle. Long turns and approved long-running tools must stay busy.
        let mut t = tracker();
        hook(&mut t, E::PromptSubmitted, 0);
        let ask = E::PermissionAsked {
            summary: "Permission: Bash sleep 30".into(),
        };
        hook(&mut t, ask, 1_000);
        let dialog = Some(Screen::Prompt {
            summary: "Do you want to proceed?".into(),
        });
        t.screen(dialog, 1_200, false);
        let mut now = 1_500;
        while now < 40_000 {
            t.output(now);
            t.glance(Some(Screen::Idle), now + 10, false);
            t.tick(now + 20, false);
            now += 500;
        }
        assert_eq!(t.status().state, S::NeedsInput);
        hook(&mut t, E::ToolFinished, 40_000);
        while now < 80_000 {
            t.output(now);
            t.glance(Some(Screen::Idle), now + 10, false);
            t.tick(now + 20, false);
            now += 500;
        }
        assert_eq!(t.status().state, S::Working);
    }

    #[test]
    fn working_without_activity_goes_stale_and_output_revives_it() {
        let mut t = tracker();
        hook(&mut t, E::PromptSubmitted, 0);
        assert!(t.tick(STALE_AFTER_MS, false));
        assert_eq!(t.status().state, S::Stale);
        assert!(t.output(STALE_AFTER_MS + 1));
        assert_eq!(t.status().state, S::Working);
    }

    #[test]
    fn heuristics_drive_state_until_the_first_hook() {
        let mut t = tracker();
        t.screen(
            Some(Screen::Prompt {
                summary: "trust this folder?".into(),
            }),
            1,
            false,
        );
        assert_eq!(t.status().state, S::NeedsInput);
        assert_eq!(t.status().ask, Some(AskKind::Screen));
        t.screen(Some(Screen::Idle), 2, false);
        assert_eq!(t.status().state, S::Idle);
        t.screen(Some(Screen::Busy), 3, false);
        t.screen(Some(Screen::Idle), 4, false);
        assert_eq!(t.status().state, S::ReviewReady);

        hook(&mut t, E::SessionStarted, 5);
        assert!(t.status().hooked);
        // Once hooked, the screen no longer sets state directly.
        t.screen(Some(Screen::Busy), 6, false);
        assert_eq!(t.status().state, S::Idle);
    }

    #[test]
    fn bell_only_counts_without_hooks() {
        let mut t = Tracker::new("generic", false, 0);
        assert!(t.apply(&E::Bell, 1, false));
        assert_eq!(t.status().ask, Some(AskKind::Bell));
        let mut t = tracker();
        hook(&mut t, E::SessionStarted, 1);
        assert!(!t.apply(&E::Bell, 2, false));
    }

    #[test]
    fn exit_is_final_and_failure_blocks() {
        let mut t = tracker();
        t.apply(&E::Exited { code: Some(2) }, 1, false);
        assert_eq!(t.status().state, S::Blocked);
        assert!(!hook(&mut t, E::PromptSubmitted, 2));
        let mut t = tracker();
        t.apply(&E::Exited { code: Some(0) }, 1, false);
        assert_eq!(t.status().state, S::Exited);
    }

    #[test]
    fn annotate_only_touches_the_named_transition() {
        let mut t = tracker();
        hook(
            &mut t,
            E::TurnEnded {
                summary: Some("Done.".into()),
            },
            1,
        );
        let seq = t.status().seq;
        assert!(t.annotate(seq, "2 files +10 -3"));
        assert_eq!(
            t.status().summary.as_deref(),
            Some("Done. · 2 files +10 -3")
        );
        assert!(!t.status().seen, "annotation is not a new transition");
        hook(&mut t, E::PromptSubmitted, 2);
        assert!(!t.annotate(seq, "late"));
    }
}
