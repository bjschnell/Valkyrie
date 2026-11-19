//! What the TUI and the web app are used for (backlog #176), with the
//! `analytics` setting on. The CLI records its own commands, so only those two
//! clients' messages are counted here, and only the ones a person asked for:
//! typing, resizing, attaching and the reads each screen polls are left out.

use crate::foreground;
use valkyrie_context::usage::{self, Use};
use valkyrie_proto::ClientMsg;

/// The feature `msg` uses, as its wire name, or `None` for plumbing.
pub(crate) fn feature(msg: &ClientMsg) -> Option<&'static str> {
    Some(match msg {
        ClientMsg::Spawn { .. } => "spawn",
        ClientMsg::Kill { .. } => "kill",
        ClientMsg::Split { .. } => "split",
        ClientMsg::Move { .. } => "move",
        ClientMsg::Rename { .. } => "rename",
        ClientMsg::Upgrade { .. } => "upgrade",
        ClientMsg::Notify { .. } => "notify",
        ClientMsg::Decide { .. } => "decide",
        ClientMsg::Review { .. } => "review",
        ClientMsg::Auto { mode: Some(_), .. } => "auto",
        ClientMsg::AddItem { .. } => "add_item",
        ClientMsg::ChangeItem { .. } => "change_item",
        ClientMsg::DeleteItem { .. } => "delete_item",
        ClientMsg::NoteItem { .. } => "note_item",
        ClientMsg::PlanOrder { .. } => "plan_order",
        ClientMsg::AnswerOrder { .. } => "answer_order",
        ClientMsg::AnswerLaunch { .. } => "answer_launch",
        ClientMsg::Supervise { .. } => "supervise",
        ClientMsg::SupervisorSession { .. } => "supervisor_session",
        ClientMsg::TakeStep { .. } => "take_step",
        ClientMsg::TakeItem { .. } => "take_item",
        ClientMsg::AdoptSession { .. } => "adopt_session",
        ClientMsg::ShipMerged { .. } => "ship_merged",
        ClientMsg::LaunchItem { .. } => "launch_item",
        ClientMsg::PruneWorktrees { .. } => "prune_worktrees",
        ClientMsg::WriteDoc { .. } => "write_doc",
        ClientMsg::EditDoc { .. } => "edit_doc",
        ClientMsg::OpenDoc { .. } => "open_doc",
        ClientMsg::CommentDoc { .. } => "comment_doc",
        ClientMsg::ResolveComment { .. } => "resolve_comment",
        ClientMsg::SaveDoc { .. } => "save_doc",
        ClientMsg::RestoreDoc { .. } => "restore_doc",
        ClientMsg::DeleteDoc { .. } => "delete_doc",
        ClientMsg::UndeleteDoc { .. } => "undelete_doc",
        ClientMsg::Digest { .. } => "digest",
        ClientMsg::Search { .. } => "search",
        ClientMsg::Note { .. } => "note",
        ClientMsg::NoteRm { .. } => "note_rm",
        ClientMsg::Wiki { .. } => "wiki",
        ClientMsg::WikiPage { .. } => "wiki_page",
        ClientMsg::WikiExplore { .. } => "wiki_explore",
        ClientMsg::Tutor { .. } => "tutor",
        ClientMsg::History { .. } => "history",
        ClientMsg::Resume { .. } => "resume",
        _ => return None,
    })
}

/// Which client a connection is: the kind it said in `Hello` (the TUI's `tui`,
/// `valk web`'s own `web`), else `web` when it comes from the bridge, whose
/// per-device connections pass on the browser's `Hello` without one.
pub(crate) fn surface(kind: Option<String>, pid: Option<i32>) -> Option<String> {
    kind.or_else(|| {
        pid.filter(|&p| p > 0)
            .is_some_and(foreground::is_web_bridge)
            .then(|| "web".to_owned())
    })
}

/// Records `msg` from a `kind` client (`tui` or `web`), if it's a feature and
/// analytics is on. A person is behind both: the web bridge acts for the
/// devices a person paired.
pub(crate) fn record(kind: &str, msg: &ClientMsg) {
    let Some(feature) = feature(msg) else { return };
    usage::record(
        &usage::default_file(),
        &Use {
            at: crate::context::now_secs(),
            surface: kind.to_owned(),
            feature: feature.to_owned(),
            by: usage::YOU.to_owned(),
            project: None,
        },
    );
}

/// The agent running above `pid`, if one does: how a `valk` command or MCP
/// server tells its uses by an agent from yours. Unlike the daemon's caller
/// check this doesn't fail closed: it only labels a count.
pub fn agent_above(pid: u32) -> Option<&'static str> {
    let pid = i32::try_from(pid).ok()?;
    foreground::ancestry(pid)
        .into_iter()
        .skip(1)
        .find_map(foreground::agent_running)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plumbing_isnt_a_feature() {
        let input: ClientMsg =
            serde_json::from_str(r#"{"t":"input","session":1,"data":[]}"#).unwrap();
        assert_eq!(feature(&input), None);
        let search: ClientMsg =
            serde_json::from_str(r#"{"t":"search","req":1,"cwd":"/r","query":"x","limit":5}"#)
                .unwrap();
        assert_eq!(feature(&search), Some("search"));
    }
}
