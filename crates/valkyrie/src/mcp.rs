//! `valk mcp`: the project's decisions as an MCP server on stdio (DESIGN §6.5), for
//! agents to search them rather than rely on the block they got at session start.
//! Line-delimited JSON-RPC 2.0. Proposing goes through the daemon like `valk decide`,
//! so from an agent it is only ever a proposal.

use anyhow::Result;
use serde_json::{Value, json};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use valkyrie_proto::client::Client;
use valkyrie_proto::{Decision, DecisionKind, DecisionStatus, NewDecision};

const PROTOCOL: &str = "2025-06-18";

pub async fn serve(client: Client) -> Result<()> {
    let cwd = std::env::current_dir()?;
    let mut lines = BufReader::new(tokio::io::stdin()).lines();
    let mut out = tokio::io::stdout();
    while let Some(line) = lines.next_line().await? {
        let Ok(msg) = serde_json::from_str::<Value>(&line) else {
            continue;
        };
        // Notifications (no id) need no answer.
        let Some(id) = msg.get("id").cloned() else {
            continue;
        };
        let reply = match handle(&client, &cwd, &msg).await {
            Ok(result) => json!({"jsonrpc": "2.0", "id": id, "result": result}),
            Err(Rpc(code, message)) => {
                json!({"jsonrpc": "2.0", "id": id, "error": {"code": code, "message": message}})
            }
        };
        out.write_all(format!("{reply}\n").as_bytes()).await?;
        out.flush().await?;
    }
    Ok(())
}

struct Rpc(i64, String);

async fn handle(client: &Client, cwd: &std::path::Path, msg: &Value) -> Result<Value, Rpc> {
    let params = &msg["params"];
    match msg["method"].as_str().unwrap_or("") {
        "initialize" => Ok(json!({
            "protocolVersion": params["protocolVersion"].as_str().unwrap_or(PROTOCOL),
            "capabilities": {"tools": {}},
            "serverInfo": {"name": "valkyrie", "version": env!("CARGO_PKG_VERSION")},
            "instructions": "This project's decisions, kept by Valkyrie and confirmed by \
                the user. Search them before choosing an approach; propose new ones the \
                user settles with you. Proposals wait on the user's review.",
        })),
        "ping" => Ok(json!({})),
        "tools/list" => Ok(json!({"tools": tools()})),
        "tools/call" => {
            let name = params["name"].as_str().unwrap_or("");
            let args = &params["arguments"];
            match call(client, cwd, name, args).await {
                Ok(text) => Ok(json!({"content": [{"type": "text", "text": text}]})),
                // A tool that failed is a result the agent reads, not a protocol error.
                Err(e) => Ok(
                    json!({"content": [{"type": "text", "text": format!("{e:#}")}], "isError": true}),
                ),
            }
        }
        other => Err(Rpc(-32601, format!("no method {other}"))),
    }
}

fn tools() -> Value {
    let kinds: Vec<&str> = DecisionKind::ALL.iter().map(|k| k.as_str()).collect();
    json!([
        {
            "name": "search_decisions",
            "description": "Search this project's decisions (conventions, constraints, gotchas, known fixes) by words. Active ones by default.",
            "inputSchema": {"type": "object", "properties": {
                "query": {"type": "string", "description": "Words to look for"},
                "include_inactive": {"type": "boolean", "description": "Also rejected, retired and superseded ones (proposals awaiting the user's review are never shown)"}
            }, "required": ["query"]}
        },
        {
            "name": "list_decisions",
            "description": "Every active decision of this project, one line each.",
            "inputSchema": {"type": "object", "properties": {}}
        },
        {
            "name": "get_decision",
            "description": "One decision in full: why, where it came from, the files it's about, and whether it may be out of date.",
            "inputSchema": {"type": "object", "properties": {"id": {"type": "integer"}}, "required": ["id"]}
        },
        {
            "name": "propose_decision",
            "description": "Propose a decision the user and you settled that should hold in future sessions. It waits on the user's review; propose sparingly.",
            "inputSchema": {"type": "object", "properties": {
                "title": {"type": "string", "description": "One line, imperative"},
                "body": {"type": "string", "description": "A sentence or two: the rule and why"},
                "kind": {"type": "string", "enum": kinds},
                "supersedes": {"type": "integer", "description": "The active decision this replaces"}
            }, "required": ["title", "body"]}
        },
        {
            "name": "project_health",
            "description": "How this project's decisions stand: counts, and which may be out of date.",
            "inputSchema": {"type": "object", "properties": {}}
        }
    ])
}

async fn call(client: &Client, cwd: &std::path::Path, name: &str, args: &Value) -> Result<String> {
    let all = || client.decisions(Some(cwd.to_path_buf()));
    match name {
        "search_decisions" => {
            let query = args["query"].as_str().unwrap_or("");
            let inactive = args["include_inactive"].as_bool().unwrap_or(false);
            let list = all().await?;
            let found = search(&list, query, inactive);
            if !found.is_empty() {
                return Ok(found.iter().map(|d| line(d)).collect::<Vec<_>>().join("\n"));
            }
            // Words miss synonyms ("package manager" vs "pnpm"); a project has tens
            // of decisions, so the agent can judge them all.
            let rest: Vec<String> = list
                .iter()
                .filter(|d| shown(d, inactive))
                .map(line)
                .collect();
            Ok(if rest.is_empty() {
                "This project has no decisions yet.".into()
            } else {
                format!(
                    "No decision has those words; here are all {}, to judge yourself:\n{}",
                    rest.len(),
                    rest.join("\n")
                )
            })
        }
        "list_decisions" => {
            let list: Vec<String> = all()
                .await?
                .iter()
                .filter(|d| d.status == DecisionStatus::Active)
                .map(line)
                .collect();
            Ok(if list.is_empty() {
                "No active decisions yet.".into()
            } else {
                list.join("\n")
            })
        }
        "get_decision" => {
            let id = args["id"].as_u64().unwrap_or(0) as u32;
            let list = all().await?;
            let d = list
                .iter()
                .find(|d| d.id == id)
                .ok_or_else(|| anyhow::anyhow!("no decision #{id}"))?;
            // Unreviewed text (an agent's, or the extractor's) never reaches an agent.
            if d.status == DecisionStatus::Proposed {
                anyhow::bail!("#{id} is a proposal the user hasn't reviewed yet");
            }
            let mut text = valkyrie_context::render(d);
            if let Some(why) = &d.fresh.review {
                text.push_str(&format!("\nMay be out of date: {why}\n"));
            }
            Ok(text)
        }
        "propose_decision" => {
            let kind = match args["kind"].as_str() {
                Some(k) => {
                    DecisionKind::parse(k).ok_or_else(|| anyhow::anyhow!("unknown kind {k}"))?
                }
                None => DecisionKind::Decision,
            };
            let d = client
                .decide(NewDecision {
                    cwd: cwd.to_path_buf(),
                    title: args["title"].as_str().unwrap_or("").to_owned(),
                    body: args["body"].as_str().unwrap_or("").to_owned(),
                    kind,
                    propose: true,
                    supersedes: args["supersedes"].as_u64().map(|n| n as u32),
                    review_every: None,
                    session: std::env::var("VALK_SESSION")
                        .ok()
                        .and_then(|s| s.parse().ok()),
                })
                .await?;
            Ok(format!(
                "Proposed #{}: {}. The user reviews it before it applies.",
                d.id, d.title
            ))
        }
        "project_health" => {
            let list = all().await?;
            let count = |s: DecisionStatus| list.iter().filter(|d| d.status == s).count();
            let mut text = format!(
                "{} active, {} proposed, {} rejected, {} retired or superseded.",
                count(DecisionStatus::Active),
                count(DecisionStatus::Proposed),
                count(DecisionStatus::Rejected),
                count(DecisionStatus::Retired) + count(DecisionStatus::Superseded)
            );
            for d in list.iter().filter(|d| d.fresh.review.is_some()) {
                text.push_str(&format!(
                    "\n#{} may be out of date: {}",
                    d.id,
                    d.fresh.review.as_deref().unwrap_or("")
                ));
            }
            Ok(text)
        }
        other => anyhow::bail!("no tool {other}"),
    }
}

fn line(d: &Decision) -> String {
    let mut text = format!("#{} [{}] {}", d.id, d.kind.as_str(), d.title);
    if d.status != DecisionStatus::Active {
        text.push_str(&format!(" ({})", d.status.as_str()));
    }
    let body = d.body.split_whitespace().collect::<Vec<_>>().join(" ");
    if !body.is_empty() {
        text.push_str(&format!(": {body}"));
    }
    text
}

/// Active decisions, and with `inactive` the ones that ended (rejected, retired,
/// superseded); never proposals, which nobody has reviewed.
fn shown(d: &Decision, inactive: bool) -> bool {
    d.status == DecisionStatus::Active || (inactive && d.status != DecisionStatus::Proposed)
}

/// Decisions with any of `query`'s words, most words matched first.
fn search<'a>(list: &'a [Decision], query: &str, inactive: bool) -> Vec<&'a Decision> {
    let words: Vec<String> = query
        .to_lowercase()
        .split(|c: char| !c.is_alphanumeric())
        .filter(|w| w.len() > 1)
        .map(str::to_owned)
        .collect();
    let mut scored: Vec<(usize, &Decision)> = list
        .iter()
        .filter(|d| shown(d, inactive))
        .filter_map(|d| {
            let text = format!("{} {} {}", d.title, d.body, d.kind.as_str()).to_lowercase();
            let score = words.iter().filter(|w| text.contains(w.as_str())).count();
            (score > 0).then_some((score, d))
        })
        .collect();
    scored.sort_by(|a, b| b.0.cmp(&a.0).then(b.1.id.cmp(&a.1.id)));
    scored.into_iter().map(|(_, d)| d).take(20).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn d(id: u32, title: &str, status: DecisionStatus) -> Decision {
        Decision {
            id,
            project: "/r".into(),
            title: title.into(),
            body: String::new(),
            kind: DecisionKind::Decision,
            status,
            created: 0,
            updated: 0,
            supersedes: None,
            superseded_by: None,
            provenance: Default::default(),
            fresh: Default::default(),
        }
    }

    #[test]
    fn search_ranks_by_words_matched_and_skips_inactive_unless_asked() {
        let list = [
            d(1, "Use pnpm for installs", DecisionStatus::Active),
            d(
                2,
                "Use pnpm workspaces for installs of tools",
                DecisionStatus::Active,
            ),
            d(3, "Use npm", DecisionStatus::Retired),
        ];
        let ids = |v: Vec<&Decision>| v.iter().map(|d| d.id).collect::<Vec<_>>();
        assert_eq!(ids(search(&list, "pnpm installs tools", false)), [2, 1]);
        assert_eq!(ids(search(&list, "npm", false)), [2, 1]);
        assert_eq!(ids(search(&list, "npm", true)), [3, 2, 1]);
        let proposed = [d(4, "Use npm everywhere", DecisionStatus::Proposed)];
        assert!(search(&proposed, "npm", true).is_empty());
        assert!(search(&list, "kubernetes", true).is_empty());
    }
}
