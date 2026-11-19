//! The tutor (backlog #226): an agent that teaches Valkyrie one lesson at a
//! time, started by `valk tutor`, the TUI's `?` and the web app. Its brief is
//! kept in the binary so it always matches the build it teaches, and the agent
//! checks each command's `--help` before teaching it.

/// What the tutor's session is called.
pub const NAME: &str = "tutor";

/// The skill `valk setup claude` installs, so `/valk-tutor` starts it in any
/// Claude Code session too.
pub const SKILL: &str = "valk-tutor";

const LESSONS: &str = include_str!("tutor.md");

/// The brief the tutor starts with. `valk` is how its agent runs Valkyrie
/// (`valk_command`): a path when `valk` isn't on PATH.
pub fn brief(valk: &str) -> String {
    // A Windows checkout may give the file CRLF line endings; the brief and the
    // skill written from it keep plain `\n` on every platform.
    LESSONS.replace("\r\n", "\n").replace("{valk}", valk)
}

/// The tutor as a Claude Code skill (`SKILL.md`).
pub fn skill(valk: &str) -> String {
    format!(
        "---\nname: {SKILL}\ndescription: Teach Valkyrie (valk) to someone new, one hands-on lesson at a time. Use when the user asks to learn Valkyrie or runs /{SKILL}.\n---\n\n{}",
        brief(valk)
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_brief_runs_the_valk_it_was_given() {
        let b = brief("/opt/v/valk");
        assert!(!b.contains("{valk}"), "a placeholder is left");
        assert!(
            b.contains("`/opt/v/valk pane split --name practice`"),
            "{b}"
        );
        // What the learner types stays `valk`.
        assert!(b.contains("`valk ls`"));
        assert!(
            !b.contains('\r'),
            "CRLF from the checkout leaked into the brief"
        );
    }

    #[test]
    fn the_skill_has_claude_codes_front_matter() {
        let s = skill("valk");
        assert!(s.starts_with("---\nname: valk-tutor\ndescription: "), "{s}");
        assert!(s.contains("\n---\n\n# Valk tutor\n"));
    }
}
