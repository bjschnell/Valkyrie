//! The plan's quota on the home header (protocol 28): how much of each window is
//! used, so there's no starting a long session on the last few percent.

use crate::theme::Theme;
use ratatui::style::Stylize;
use ratatui::text::{Line, Span};
use valkyrie_proto::{Quota, QuotaLimit};

/// Past this the reading says how old it is: it's only as new as the last Claude
/// session to draw its status line.
const FRESH_MS: u64 = 5 * 60 * 1000;
/// From here a window's share is a warning, and from `FULL` it needs you.
const LOW: u8 = 75;
const FULL: u8 = 90;

/// A window's short name: `5h`, `wk`, `wk opus`, and Claude Code's own for the rest.
pub(crate) fn label(name: &str) -> String {
    match name {
        "five_hour" => "5h".into(),
        "seven_day" => "wk".into(),
        _ => match name.strip_prefix("seven_day_") {
            Some(rest) => format!("wk {}", rest.replace('_', " ")),
            None => name.replace('_', " "),
        },
    }
}

/// ` claude 5h 26% · wk 10% `, the shares coloured by how full they are; a window
/// past its reset says so rather than its old share.
pub(crate) fn line(quota: &Quota, now_ms: u64, t: &Theme) -> Line<'static> {
    let at_ms = quota.at * 1000;
    let stale = now_ms.saturating_sub(at_ms) > FRESH_MS;
    let mut spans: Vec<Span> = vec![" claude ".fg(t.muted)];
    for (i, limit) in quota.limits.iter().enumerate() {
        if i > 0 {
            spans.push(" · ".fg(t.muted));
        }
        spans.push(format!("{} ", label(&limit.name)).fg(t.muted));
        spans.push(share(limit, now_ms, stale, t));
    }
    if stale {
        spans.push(format!(" · {} ago", crate::age(at_ms, now_ms)).fg(t.muted));
    }
    spans.push(" ".into());
    Line::from(spans)
}

fn share(limit: &QuotaLimit, now_ms: u64, stale: bool, t: &Theme) -> Span<'static> {
    if limit.resets_at.is_some_and(|at| at * 1000 <= now_ms) {
        return "reset".fg(t.muted);
    }
    let text = format!("{}%", limit.used);
    if stale {
        return text.fg(t.muted);
    }
    match limit.used {
        FULL.. => text.fg(t.blocked).bold(),
        LOW.. => text.fg(t.needs).bold(),
        _ => text.fg(t.fg),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn quota(at: u64, five: u8, resets: u64) -> Quota {
        Quota {
            at,
            limits: vec![
                QuotaLimit {
                    name: "five_hour".into(),
                    used: five,
                    resets_at: Some(resets),
                },
                QuotaLimit {
                    name: "seven_day_opus".into(),
                    used: 10,
                    resets_at: None,
                },
            ],
        }
    }

    fn text(line: &Line) -> String {
        line.spans.iter().map(|s| s.content.as_ref()).collect()
    }

    #[test]
    fn says_each_windows_share_and_colours_a_full_one() {
        let t = crate::theme::THEMES[0];
        let line = line(&quota(1000, 92, 5000), 1_060_000, t);
        assert_eq!(text(&line), " claude 5h 92% · wk opus 10% ");
        let five = line.spans.iter().find(|s| s.content == "92%").unwrap();
        assert_eq!(five.style.fg, Some(t.blocked));
    }

    #[test]
    fn an_old_reading_says_its_age_and_a_passed_reset_says_reset() {
        let t = crate::theme::THEMES[0];
        // Read at 1000 s, looked at an hour on, the five-hour window reset at 2000 s.
        let line = line(&quota(1000, 92, 2000), 4_600_000, t);
        assert_eq!(text(&line), " claude 5h reset · wk opus 10% · 1h ago ");
    }

    #[test]
    fn labels_are_short() {
        assert_eq!(label("five_hour"), "5h");
        assert_eq!(label("seven_day"), "wk");
        assert_eq!(label("seven_day_oauth_apps"), "wk oauth apps");
        assert_eq!(label("daily"), "daily");
    }
}
