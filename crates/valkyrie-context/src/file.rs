//! One decision as a markdown file: `key: value` front matter between `---` lines,
//! then `# title` and the body. Hand-rolled so a person can edit the files and so
//! the hook has nothing heavy to load.

use std::path::PathBuf;
use valkyrie_proto::{Decision, DecisionKind, DecisionStatus, Provenance};

pub fn render(d: &Decision) -> String {
    let mut out = String::from("---\n");
    let mut field = |k: &str, v: &str| {
        if !v.is_empty() {
            out.push_str(&format!("{k}: {}\n", one_line(v)));
        }
    };
    field("id", &d.id.to_string());
    field("kind", d.kind.as_str());
    field("status", d.status.as_str());
    field("created", &d.created.to_string());
    field("updated", &d.updated.to_string());
    field("supersedes", &opt(d.supersedes));
    field("superseded_by", &opt(d.superseded_by));
    field("by", &d.provenance.by);
    field("session", d.provenance.session.as_deref().unwrap_or(""));
    field(
        "conversation",
        d.provenance.conversation.as_deref().unwrap_or(""),
    );
    field("commit", d.provenance.commit.as_deref().unwrap_or(""));
    let cwd = d.provenance.cwd.as_ref().map(|p| p.to_string_lossy());
    field("cwd", cwd.as_deref().unwrap_or(""));
    if d.fresh.confirmed > 0 {
        field("confirmed", &d.fresh.confirmed.to_string());
    }
    field("review_every", &opt(d.fresh.review_every));
    field("anchors", &d.fresh.anchors.join(", "));
    out.push_str("---\n");
    out.push_str(&format!("# {}\n", one_line(&d.title)));
    let body = d.body.trim();
    if !body.is_empty() {
        out.push('\n');
        out.push_str(body);
        out.push('\n');
    }
    out
}

/// Reads a decision file back. `project` isn't stored in the file: it's the
/// directory the file is in.
pub fn parse(text: &str, project: PathBuf) -> Option<Decision> {
    let rest = text.strip_prefix("---\n")?;
    let (front, rest) = rest.split_once("\n---\n")?;
    let mut d = Decision {
        id: 0,
        project,
        title: String::new(),
        body: String::new(),
        kind: DecisionKind::default(),
        status: DecisionStatus::default(),
        created: 0,
        updated: 0,
        supersedes: None,
        superseded_by: None,
        provenance: Provenance::default(),
        fresh: Default::default(),
    };
    for line in front.lines() {
        let Some((k, v)) = line.split_once(':') else {
            continue;
        };
        let v = v.trim();
        let text = || (!v.is_empty()).then(|| v.to_owned());
        match k.trim() {
            "id" => d.id = v.parse().ok()?,
            "kind" => d.kind = DecisionKind::parse(v).unwrap_or_default(),
            "status" => d.status = DecisionStatus::parse(v)?,
            "created" => d.created = v.parse().unwrap_or(0),
            "updated" => d.updated = v.parse().unwrap_or(0),
            "supersedes" => d.supersedes = v.parse().ok(),
            "superseded_by" => d.superseded_by = v.parse().ok(),
            "by" => d.provenance.by = v.to_owned(),
            "session" => d.provenance.session = text(),
            "conversation" => d.provenance.conversation = text(),
            "commit" => d.provenance.commit = text(),
            "cwd" => d.provenance.cwd = text().map(PathBuf::from),
            "confirmed" => d.fresh.confirmed = v.parse().unwrap_or(0),
            "review_every" => d.fresh.review_every = v.parse().ok(),
            "anchors" => {
                d.fresh.anchors = v
                    .split(',')
                    .map(str::trim)
                    .filter(|a| !a.is_empty())
                    .map(str::to_owned)
                    .collect()
            }
            _ => {}
        }
    }
    let rest = rest.trim_start_matches('\n');
    let (title, body) = match rest.split_once('\n') {
        Some((t, b)) => (t, b),
        None => (rest, ""),
    };
    d.title = title.strip_prefix("# ").unwrap_or(title).trim().to_owned();
    d.body = body.trim().to_owned();
    (d.id > 0 && !d.title.is_empty()).then_some(d)
}

/// `0007-use-sqlite-for-the-index.md`
pub fn name(d: &Decision) -> String {
    let mut slug = String::new();
    for c in d.title.chars().flat_map(char::to_lowercase) {
        if c.is_ascii_alphanumeric() {
            slug.push(c);
        } else if !slug.ends_with('-') && !slug.is_empty() {
            slug.push('-');
        }
        if slug.len() >= 48 {
            break;
        }
    }
    let slug = slug.trim_end_matches('-');
    if slug.is_empty() {
        format!("{:04}.md", d.id)
    } else {
        format!("{:04}-{slug}.md", d.id)
    }
}

fn opt(v: Option<u32>) -> String {
    v.map(|v| v.to_string()).unwrap_or_default()
}

pub(crate) fn one_line(s: &str) -> String {
    s.split_whitespace().collect::<Vec<_>>().join(" ")
}

#[cfg(test)]
mod tests {
    use super::*;

    pub(crate) fn sample() -> Decision {
        Decision {
            id: 7,
            project: PathBuf::from("/r"),
            title: "Use SQLite\nfor the index".into(),
            body: "Markdown stays canonical.\n\nSQLite is a cache.".into(),
            kind: DecisionKind::Constraint,
            status: DecisionStatus::Active,
            created: 100,
            updated: 200,
            supersedes: Some(3),
            superseded_by: None,
            provenance: Provenance {
                by: "claude".into(),
                session: Some("valkyrie".into()),
                conversation: Some("abc-123".into()),
                commit: Some("deadbeef".into()),
                cwd: Some(PathBuf::from("/r/sub")),
            },
            fresh: valkyrie_proto::Freshness {
                confirmed: 300,
                review_every: Some(30),
                anchors: vec!["src/a.rs".into(), "Cargo.toml".into()],
                review: None,
            },
        }
    }

    #[test]
    fn round_trips() {
        let d = sample();
        let back = parse(&render(&d), d.project.clone()).unwrap();
        assert_eq!(back.title, "Use SQLite for the index");
        assert_eq!(
            back,
            Decision {
                title: back.title.clone(),
                ..d
            }
        );
    }

    #[test]
    fn hand_edited_files_parse_and_broken_ones_are_skipped() {
        let text = "---\nid: 2\nstatus: proposed\nnote: ignored\n---\n\n# Tabs, not spaces\n";
        let d = parse(text, PathBuf::from("/r")).unwrap();
        assert_eq!(
            (d.id, d.title.as_str(), d.body.as_str()),
            (2, "Tabs, not spaces", "")
        );
        assert_eq!(d.kind, DecisionKind::Decision);
        assert!(parse("# no front matter", PathBuf::new()).is_none());
        assert!(parse("---\nid: 1\nstatus: nope\n---\n# t\n", PathBuf::new()).is_none());
    }

    #[test]
    fn file_names_are_numbered_slugs() {
        assert_eq!(name(&sample()), "0007-use-sqlite-for-the-index.md");
        let mut d = sample();
        d.title = "!!!".into();
        assert_eq!(name(&d), "0007.md");
    }
}
