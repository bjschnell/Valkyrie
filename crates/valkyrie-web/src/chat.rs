//! The Chat view (DESIGN §8.7): a session's agent conversation as messages and tool
//! cards, read from the agent's own transcript (Claude's `~/.claude/projects/…/<id>.jsonl`,
//! Codex's `~/.codex/sessions/…/rollout-….jsonl`). The daemon says where the file is.
//! Neither format is a documented interface, so both are read defensively: a line
//! that doesn't parse, or that isn't understood, is skipped.
//!
//! Ported from Alice's mappers.

use anyhow::Result;
use serde::Serialize;
use serde_json::{Value, json};
use std::collections::HashMap;
use std::fs::File;
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::time::Duration;
use tokio::sync::mpsc;
use valkyrie_proto::SessionId;
use valkyrie_proto::client::Client;

/// One line on a collapsed card.
const MAX_SUMMARY: usize = 120;
/// A command, diff or file written, as the card shows it opened.
const MAX_DETAIL: usize = 8_000;
/// A tool's output.
const MAX_RESULT: usize = 4_000;
/// Something said or typed.
const MAX_TEXT: usize = 32_000;
/// How far back opening a chat reads. Transcripts reach tens of MB.
const INITIAL_BYTES: u64 = 6 * 1024 * 1024;
/// Items sent when a chat opens; older ones aren't loaded.
const INITIAL_ITEMS: usize = 400;
/// Read per poll, so a burst doesn't stall the connection.
const READ_CHUNK: usize = 2 * 1024 * 1024;
/// A line longer than this is skipped (a pasted image, say).
const MAX_LINE: usize = 16 * 1024 * 1024;
/// How often the transcript is read for new lines.
const POLL: Duration = Duration::from_millis(600);
/// How often the daemon is asked where the transcript is (`/clear` moves it).
const RELOOK_EVERY: u32 = 5;

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(tag = "k", rename_all = "snake_case")]
pub enum Item {
    /// What you typed.
    User {
        id: String,
        text: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        at: Option<String>,
    },
    /// What the agent said.
    Say {
        id: String,
        text: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        at: Option<String>,
    },
    /// One tool call. Sent again, under the same id, when its result comes in.
    Tool {
        id: String,
        tool: String,
        summary: String,
        status: Status,
        detail: Detail,
    },
    /// A turn's end or a break in the conversation: "Worked for 2m 4s", "Interrupted".
    Mark {
        id: String,
        text: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        at: Option<String>,
    },
}

impl Item {
    pub fn id(&self) -> &str {
        match self {
            Item::User { id, .. }
            | Item::Say { id, .. }
            | Item::Tool { id, .. }
            | Item::Mark { id, .. } => id,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Status {
    Running,
    Ok,
    Error,
    /// Refused: by you at a prompt, or by auto mode. The agent carried on without it.
    Denied,
}

/// What opening a card shows.
#[derive(Debug, Clone, Default, PartialEq, Serialize)]
pub struct Detail {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub file: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub command: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub diff: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub content: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub result: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Agent {
    Claude,
    Codex,
}

impl Agent {
    /// Codex names its transcripts `rollout-<time>-<id>.jsonl`.
    pub fn of(path: &Path) -> Agent {
        let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("");
        if name.starts_with("rollout-") {
            Agent::Codex
        } else {
            Agent::Claude
        }
    }
}

/// What Claude Code writes as a user turn when Esc stops it.
const CLAUDE_INTERRUPT: &str = "[Request interrupted by user";

/// Turns transcript lines into items. Keeps each tool call, to update it when its
/// result arrives.
pub struct Mapper {
    agent: Agent,
    /// The session's directory; paths inside it read relative to it.
    repo: String,
    tools: HashMap<String, Item>,
    marks: u64,
}

impl Mapper {
    pub fn new(agent: Agent, repo: &str) -> Self {
        Self {
            agent,
            repo: repo.trim_end_matches('/').to_owned(),
            tools: HashMap::new(),
            marks: 0,
        }
    }

    pub fn line(&mut self, line: &[u8]) -> Vec<Item> {
        let Ok(event) = serde_json::from_slice::<Value>(line) else {
            return Vec::new();
        };
        if !event.is_object() {
            return Vec::new();
        }
        match self.agent {
            Agent::Claude => self.claude(&event),
            Agent::Codex => self.codex(&event),
        }
    }

    fn mark(&mut self, text: String, at: Option<String>) -> Item {
        self.marks += 1;
        Item::Mark {
            id: format!("m{}", self.marks),
            text,
            at,
        }
    }

    fn tool(&mut self, id: String, name: &str, args: &Value) -> Item {
        let (summary, detail) = summarise(name, args, &self.repo);
        let item = Item::Tool {
            id: id.clone(),
            tool: name.to_owned(),
            summary,
            status: Status::Running,
            detail,
        };
        self.tools.insert(id, item.clone());
        item
    }

    /// The call `id` finished; its card, updated, if it was seen.
    fn finish(&mut self, id: &str, status: Status, output: &str) -> Option<Item> {
        let mut item = self.tools.remove(id)?;
        if let Item::Tool {
            status: s, detail, ..
        } = &mut item
        {
            *s = status;
            let output = output.trim_end();
            if !output.is_empty() {
                detail.result = Some(clip(output, MAX_RESULT));
            }
        }
        Some(item)
    }

    // -- Claude Code -------------------------------------------------------------
    //
    // One content block per line, `assistant` and `user` lines as in stream-json.
    // A typed prompt is a `user` line with string content; tool results are `user`
    // lines too. Slash commands, reminders and relayed messages are `user` lines
    // starting with `<`, or marked `isMeta`, or with a non-human `origin`.

    fn claude(&mut self, event: &Value) -> Vec<Item> {
        if event["isSidechain"] == true {
            return Vec::new();
        }
        let at = event["timestamp"].as_str().map(str::to_owned);
        let uuid = event["uuid"].as_str().unwrap_or("");
        match event["type"].as_str() {
            Some("assistant") => {
                let mut out = Vec::new();
                for (i, block) in content(event).iter().enumerate() {
                    match block["type"].as_str() {
                        Some("text") => {
                            let text = block["text"].as_str().unwrap_or("").trim();
                            if !text.is_empty() {
                                out.push(Item::Say {
                                    id: format!("{uuid}:{i}"),
                                    text: clip(text, MAX_TEXT),
                                    at: at.clone(),
                                });
                            }
                        }
                        Some("tool_use") => {
                            let id = block["id"].as_str().unwrap_or(uuid).to_owned();
                            let name = block["name"].as_str().unwrap_or("tool");
                            out.push(self.tool(id, name, &block["input"]));
                        }
                        _ => {}
                    }
                }
                out
            }
            Some("user") => {
                if event["isMeta"] == true || event["isCompactSummary"] == true || !human(event) {
                    return Vec::new();
                }
                let mut out = Vec::new();
                let mut typed = Vec::new();
                for block in content(event) {
                    match block["type"].as_str() {
                        Some("tool_result") => {
                            let text = result_text(&block["content"]);
                            let status = if block["is_error"] != true {
                                Status::Ok
                            } else if denied(&text) {
                                Status::Denied
                            } else {
                                Status::Error
                            };
                            let id = block["tool_use_id"].as_str().unwrap_or("");
                            out.extend(self.finish(id, status, &text));
                        }
                        Some("text") => {
                            let text = block["text"].as_str().unwrap_or("").trim();
                            if text.starts_with(CLAUDE_INTERRUPT) {
                                out.push(self.mark("Interrupted".into(), at.clone()));
                            } else if typed_by_you(text) {
                                typed.push(text.to_owned());
                            }
                        }
                        Some("image") => typed.push("[image]".into()),
                        _ => {}
                    }
                }
                if !typed.is_empty() {
                    out.push(Item::User {
                        id: uuid.to_owned(),
                        text: clip(&typed.join("\n"), MAX_TEXT),
                        at,
                    });
                }
                out
            }
            // A message typed while it worked, queued for its next step: written
            // only as this, never as a `user` line.
            Some("attachment") => {
                let att = &event["attachment"];
                let text = att["prompt"].as_str().unwrap_or("").trim();
                if att["type"] != "queued_command"
                    || att["commandMode"] != "prompt"
                    || !human(event)
                    || !typed_by_you(text)
                {
                    return Vec::new();
                }
                vec![Item::User {
                    id: uuid.to_owned(),
                    text: clip(text, MAX_TEXT),
                    at,
                }]
            }
            Some("system") => match event["subtype"].as_str() {
                Some("turn_duration") => match event["durationMs"].as_u64() {
                    Some(ms) => vec![self.mark(format!("Worked for {}", duration(ms)), at)],
                    None => Vec::new(),
                },
                Some("compact_boundary") => vec![self.mark("Conversation compacted".into(), at)],
                _ => Vec::new(),
            },
            _ => Vec::new(),
        }
    }

    // -- Codex -------------------------------------------------------------------
    //
    // Every line is `{type, payload}`. `response_item` lines are the raw model
    // conversation and are skipped; the same activity is in `event_msg` lines, one
    // `item_completed` per thing that happened, which is a card's level. Items come
    // finished, so a Codex card never shows as running.

    fn codex(&mut self, event: &Value) -> Vec<Item> {
        if event["type"] != "event_msg" {
            return Vec::new();
        }
        let at = event["timestamp"].as_str().map(str::to_owned);
        let payload = &event["payload"];
        match payload["type"].as_str() {
            Some("task_complete") => match payload["duration_ms"].as_u64() {
                Some(ms) => vec![self.mark(format!("Worked for {}", duration(ms)), at)],
                None => Vec::new(),
            },
            Some("turn_aborted") => {
                let text = if payload["reason"] == "interrupted" {
                    "Interrupted"
                } else {
                    "Stopped"
                };
                vec![self.mark(text.into(), at)]
            }
            Some("item_completed") => self.codex_item(&payload["item"], at),
            _ => Vec::new(),
        }
    }

    fn codex_item(&mut self, item: &Value, at: Option<String>) -> Vec<Item> {
        let id = item["id"].as_str().unwrap_or("").to_owned();
        let texts = |item: &Value| {
            item["content"]
                .as_array()
                .map(|parts| {
                    parts
                        .iter()
                        .filter_map(|p| p["text"].as_str())
                        .collect::<Vec<_>>()
                        .join("\n")
                })
                .unwrap_or_default()
        };
        let finished = |status: &Value| match status.as_str() {
            Some("failed") => Status::Error,
            Some("declined") => Status::Denied,
            _ => Status::Ok,
        };
        match item["type"].as_str().unwrap_or("") {
            "UserMessage" => {
                let text = texts(item);
                let text = text.trim();
                if !typed_by_you(text) {
                    return Vec::new();
                }
                vec![Item::User {
                    id,
                    text: clip(text, MAX_TEXT),
                    at,
                }]
            }
            "AgentMessage" => {
                let text = texts(item);
                let text = text.trim();
                if text.is_empty() {
                    return Vec::new();
                }
                vec![Item::Say {
                    id,
                    text: clip(text, MAX_TEXT),
                    at,
                }]
            }
            "CommandExecution" => {
                let command = codex_command(&item["command"]);
                self.tool(id.clone(), "Bash", &json!({ "command": command }));
                let code = item["exit_code"].as_i64();
                let mut status = finished(&item["status"]);
                let mut output = item["aggregated_output"]
                    .as_str()
                    .or(item["formatted_output"].as_str())
                    .unwrap_or("")
                    .to_owned();
                if let Some(code) = code.filter(|&c| c != 0) {
                    output = format!("exit {code}\n{output}");
                    if status == Status::Ok {
                        status = Status::Error;
                    }
                }
                self.finish(&id, status, &output).into_iter().collect()
            }
            "FileChange" => {
                let status = finished(&item["status"]);
                let Some(changes) = item["changes"].as_object() else {
                    return Vec::new();
                };
                let mut out = Vec::new();
                for (n, (path, change)) in changes.iter().enumerate() {
                    let card = format!("{id}-{n}");
                    let file = rel(path, &self.repo);
                    match change["type"].as_str() {
                        Some("update") => {
                            let diff = change["unified_diff"].as_str().unwrap_or("");
                            let (added, removed) = count_diff(diff);
                            self.tools.insert(
                                card.clone(),
                                Item::Tool {
                                    id: card.clone(),
                                    tool: "Edit".into(),
                                    summary: one_line(&format!("{file} (+{added}/-{removed})")),
                                    status: Status::Running,
                                    detail: Detail {
                                        file: Some(file),
                                        diff: Some(clip(diff, MAX_DETAIL)),
                                        ..Detail::default()
                                    },
                                },
                            );
                        }
                        Some("delete") => {
                            self.tools.insert(
                                card.clone(),
                                Item::Tool {
                                    id: card.clone(),
                                    tool: "Delete".into(),
                                    summary: one_line(&file),
                                    status: Status::Running,
                                    detail: Detail {
                                        file: Some(file),
                                        ..Detail::default()
                                    },
                                },
                            );
                        }
                        _ => {
                            let args = json!({"file_path": path, "content": change["content"]});
                            self.tool(card.clone(), "Write", &args);
                        }
                    }
                    out.extend(self.finish(&card, status, ""));
                }
                out
            }
            "ImageView" => {
                let path = item["path"].as_str().unwrap_or("");
                let path = path.strip_prefix("file://").unwrap_or(path);
                self.tool(id.clone(), "View", &json!({ "file_path": path }));
                self.finish(&id, Status::Ok, "").into_iter().collect()
            }
            "Extension" if item["kind"] == "web.search" => {
                self.tool(id.clone(), "WebSearch", &json!({ "query": item["query"] }));
                self.finish(&id, Status::Ok, "").into_iter().collect()
            }
            "McpToolCall" => {
                let server = item["server"].as_str().unwrap_or("mcp");
                let tool = item["tool"].as_str().unwrap_or("tool");
                let summary = item["arguments"]["title"]
                    .as_str()
                    .map(|t| json!({ "title": t }))
                    .unwrap_or_else(|| item["arguments"].clone());
                self.tool(id.clone(), &format!("{server}.{tool}"), &summary);
                let status = if item["result"]["isError"] == true {
                    Status::Error
                } else {
                    finished(&item["status"])
                };
                let output = result_text(&item["result"]["content"]);
                self.finish(&id, status, &output).into_iter().collect()
            }
            "ContextCompaction" => vec![self.mark("Conversation compacted".into(), at)],
            // Its reasoning is a scratchpad, not something it said.
            "Reasoning" | "" => Vec::new(),
            other => {
                let other = other.to_owned();
                self.tool(id.clone(), &other, item);
                self.finish(&id, finished(&item["status"]), "")
                    .into_iter()
                    .collect()
            }
        }
    }
}

/// `["/usr/bin/bash", "-lc", "<the command>"]` reads as the command.
fn codex_command(command: &Value) -> String {
    match command {
        Value::String(s) => s.clone(),
        Value::Array(argv) => {
            let argv: Vec<&str> = argv.iter().filter_map(Value::as_str).collect();
            match argv.as_slice() {
                [.., flag, command] if *flag == "-lc" || *flag == "-c" => (*command).to_owned(),
                _ => argv.join(" "),
            }
        }
        _ => String::new(),
    }
}

fn content(event: &Value) -> Vec<Value> {
    match &event["message"]["content"] {
        Value::String(text) => vec![json!({"type": "text", "text": text})],
        Value::Array(blocks) => blocks.iter().filter(|b| b.is_object()).cloned().collect(),
        _ => Vec::new(),
    }
}

/// Lines with a non-human `origin` are another agent's hand-back.
fn human(event: &Value) -> bool {
    match event["origin"]["kind"].as_str() {
        None => true,
        Some(kind) => kind == "human",
    }
}

/// Tagged lines (`<command-name>`, `<task-notification>`, reminders) and relayed
/// messages aren't something you typed.
fn typed_by_you(text: &str) -> bool {
    !text.is_empty()
        && !text.starts_with('<')
        && !text.starts_with("Another Claude session sent a message")
}

/// A refused call's result: rejected at the prompt, or by auto mode's checks.
fn denied(text: &str) -> bool {
    let first = text.lines().next().unwrap_or("");
    first.starts_with("Permission for this")
        || first.contains("doesn't want to proceed")
        || first.contains("was rejected")
}

fn result_text(content: &Value) -> String {
    match content {
        Value::String(s) => s.clone(),
        Value::Array(parts) => parts
            .iter()
            .map(|p| match p["type"].as_str() {
                Some("text") => p["text"].as_str().unwrap_or("").to_owned(),
                Some(kind) => format!("[{kind}]"),
                None => String::new(),
            })
            .collect::<Vec<_>>()
            .join("\n"),
        Value::Null => String::new(),
        other => other.to_string(),
    }
}

/// One line on a collapsed card, and what opening it shows. Per tool, because the
/// useful line differs: for an edit it's the file and how much changed, for a
/// command the command. Else the first string argument, which is right often enough.
fn summarise(name: &str, args: &Value, repo: &str) -> (String, Detail) {
    let s = |key: &str| args[key].as_str().unwrap_or("");
    let file = |key: &str| rel(s(key), repo);
    match name {
        "Edit" | "MultiEdit" => {
            let file = file("file_path");
            let edits = match name {
                "MultiEdit" => args["edits"].as_array().cloned().unwrap_or_default(),
                _ => vec![args.clone()],
            };
            let (mut chunks, mut added, mut removed) = (Vec::new(), 0, 0);
            for edit in &edits {
                let old = edit["old_string"].as_str().unwrap_or("");
                let new = edit["new_string"].as_str().unwrap_or("");
                let diff = unified(old, new, &file);
                let (a, r) = count_diff(&diff);
                chunks.push(diff);
                added += a;
                removed += r;
            }
            (
                one_line(&format!("{file} (+{added}/-{removed})")),
                Detail {
                    diff: Some(clip(&chunks.join("\n"), MAX_DETAIL)),
                    file: Some(file),
                    ..Detail::default()
                },
            )
        }
        "Write" => {
            let file = file("file_path");
            let content = s("content");
            (
                one_line(&format!("{file} (+{} lines)", content.lines().count())),
                Detail {
                    content: Some(clip(content, MAX_DETAIL)),
                    file: Some(file),
                    ..Detail::default()
                },
            )
        }
        "NotebookEdit" => {
            let file = file("notebook_path");
            (
                one_line(&file),
                Detail {
                    content: Some(clip(s("new_source"), MAX_DETAIL)),
                    file: Some(file),
                    ..Detail::default()
                },
            )
        }
        "Bash" => (
            one_line(s("command")),
            Detail {
                command: Some(clip(s("command"), MAX_DETAIL)),
                ..Detail::default()
            },
        ),
        "Read" | "View" => (one_line(&file("file_path")), Detail::default()),
        "Grep" | "Glob" => {
            let pattern = s("pattern");
            let line = match file("path") {
                place if place.is_empty() => pattern.to_owned(),
                place => format!("{pattern} in {place}"),
            };
            (one_line(&line), Detail::default())
        }
        "Task" | "Agent" => {
            let what = Some(s("description"))
                .filter(|d| !d.is_empty())
                .unwrap_or(s("prompt"));
            (one_line(what), Detail::default())
        }
        "TodoWrite" => {
            let n = args["todos"].as_array().map_or(0, Vec::len);
            (format!("{n} todos"), Detail::default())
        }
        "AskUserQuestion" => {
            let q = args["questions"][0]["question"].as_str().unwrap_or("");
            (one_line(q), Detail::default())
        }
        _ => {
            let first = args
                .as_object()
                .and_then(|map| {
                    map.iter()
                        .filter(|(k, _)| !matches!(k.as_str(), "id" | "type" | "status"))
                        .find_map(|(_, v)| v.as_str().filter(|v| !v.trim().is_empty()))
                })
                .unwrap_or("");
            (one_line(first), Detail::default())
        }
    }
}

/// An edit as a unified diff, two lines of context.
fn unified(old: &str, new: &str, name: &str) -> String {
    similar::TextDiff::from_lines(old, new)
        .unified_diff()
        .context_radius(2)
        .header(&format!("a/{name}"), &format!("b/{name}"))
        .to_string()
        .trim_end()
        .to_owned()
}

fn count_diff(diff: &str) -> (usize, usize) {
    diff.lines().fold((0, 0), |(a, r), line| {
        if line.starts_with('+') && !line.starts_with("+++") {
            (a + 1, r)
        } else if line.starts_with('-') && !line.starts_with("---") {
            (a, r + 1)
        } else {
            (a, r)
        }
    })
}

/// A path as it reads in the repo.
fn rel(path: &str, repo: &str) -> String {
    match path.strip_prefix(repo).and_then(|p| p.strip_prefix('/')) {
        Some(inside) if !repo.is_empty() => inside.to_owned(),
        _ => path.to_owned(),
    }
}

fn clip(text: &str, limit: usize) -> String {
    if text.len() <= limit {
        return text.to_owned();
    }
    let mut end = limit;
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    format!("{}…", &text[..end])
}

fn one_line(text: &str) -> String {
    clip(
        &text.split_whitespace().collect::<Vec<_>>().join(" "),
        MAX_SUMMARY,
    )
}

fn duration(ms: u64) -> String {
    let s = (ms + 500) / 1000;
    match s {
        0..60 => format!("{s}s"),
        60..3600 => format!("{}m {}s", s / 60, s % 60),
        _ => format!("{}h {}m", s / 3600, s % 3600 / 60),
    }
}

/// Items as a list shows them: each at its first place, in its latest version.
pub fn collapse(items: Vec<Item>) -> Vec<Item> {
    let mut at: HashMap<String, usize> = HashMap::new();
    let mut out: Vec<Item> = Vec::new();
    for item in items {
        match at.get(item.id()) {
            Some(&i) => out[i] = item,
            None => {
                at.insert(item.id().to_owned(), out.len());
                out.push(item);
            }
        }
    }
    out
}

/// A transcript read as it grows.
pub struct Tail {
    file: File,
    /// Where reading began: past 0, the start of the conversation wasn't read.
    start: u64,
    offset: u64,
    partial: Vec<u8>,
    /// In the middle of a line too long to keep: skip to its end.
    skipping: bool,
    mapper: Mapper,
}

impl Tail {
    /// Opens `path`, starting at most `INITIAL_BYTES` from its end.
    pub fn open(path: &Path, repo: &str) -> Result<Tail> {
        let file = File::open(path)?;
        let len = file.metadata()?.len();
        let offset = len.saturating_sub(INITIAL_BYTES);
        Ok(Tail {
            file,
            start: offset,
            offset,
            partial: Vec::new(),
            // Mid-file: the first line is cut, so it's dropped.
            skipping: offset > 0,
            mapper: Mapper::new(Agent::of(path), repo),
        })
    }

    /// Whatever was written since the last read, up to `READ_CHUNK`.
    pub fn read(&mut self) -> Result<Vec<Item>> {
        self.file.seek(SeekFrom::Start(self.offset))?;
        let mut buf = Vec::new();
        (&mut self.file)
            .take(READ_CHUNK as u64)
            .read_to_end(&mut buf)?;
        self.offset += buf.len() as u64;
        let mut items = Vec::new();
        let mut rest = buf.as_slice();
        while let Some(end) = rest.iter().position(|&b| b == b'\n') {
            let (line, after) = rest.split_at(end);
            rest = &after[1..];
            if std::mem::take(&mut self.skipping) {
                self.partial.clear();
                continue;
            }
            if self.partial.is_empty() {
                items.extend(self.mapper.line(line));
            } else {
                self.partial.extend_from_slice(line);
                let whole = std::mem::take(&mut self.partial);
                items.extend(self.mapper.line(&whole));
            }
        }
        if !self.skipping {
            self.partial.extend_from_slice(rest);
            if self.partial.len() > MAX_LINE {
                self.partial.clear();
                self.skipping = true;
            }
        }
        Ok(items)
    }

    /// Reads until caught up: what opening a chat shows.
    pub fn read_all(&mut self) -> Result<Vec<Item>> {
        let mut items = Vec::new();
        loop {
            let before = self.offset;
            items.extend(self.read()?);
            if self.offset == before {
                return Ok(items);
            }
        }
    }
}

/// Follows one session's chat for a browser, sending `{"t":"chat",…}` messages to
/// `tx` until it closes. A reset message replaces what the browser has; others add
/// to it, replacing items with the same id.
pub async fn follow(socket: PathBuf, session: SessionId, tx: mpsc::Sender<Value>) {
    while !tx.is_closed() {
        match follow_once(&socket, session, &tx).await {
            Ok(()) => return,
            Err(e) => tracing::debug!(session, "chat: {e:#}"),
        }
        tokio::time::sleep(Duration::from_secs(2)).await;
    }
}

async fn follow_once(socket: &Path, session: SessionId, tx: &mpsc::Sender<Value>) -> Result<()> {
    let (client, _pushes) = Client::connect(socket).await?;
    let mut tail: Option<(PathBuf, Tail)> = None;
    let mut told_missing = false;
    let mut tick = tokio::time::interval(POLL);
    tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    for n in 0u32.. {
        tick.tick().await;
        if n % RELOOK_EVERY == 0 {
            let info = client.list().await?.into_iter().find(|s| s.id == session);
            let Some(info) = info else {
                tx.send(json!({"t": "chat", "session": session, "reset": true,
                               "items": [], "gone": true}))
                    .await?;
                return Ok(());
            };
            let current = tail.as_ref().map(|(path, _)| path);
            match info.chat {
                Some(path) if current != Some(&path) => {
                    let repo = info.cwd.to_string_lossy().into_owned();
                    let opened = path.clone();
                    let (t, items) = tokio::task::spawn_blocking(move || {
                        let mut t = Tail::open(&opened, &repo)?;
                        let items = t.read_all()?;
                        anyhow::Ok((t, items))
                    })
                    .await??;
                    let mut items = collapse(items);
                    let older = items.len().saturating_sub(INITIAL_ITEMS);
                    items.drain(..older);
                    tx.send(json!({"t": "chat", "session": session, "reset": true,
                                   "items": items, "more": older > 0 || t.start > 0}))
                        .await?;
                    tail = Some((path, t));
                    continue;
                }
                None if tail.is_none() && !told_missing => {
                    told_missing = true;
                    tx.send(json!({"t": "chat", "session": session, "reset": true,
                                   "items": [], "missing": true}))
                        .await?;
                }
                _ => {}
            }
        }
        if let Some((path, t)) = tail.take() {
            let (t, items) = tokio::task::spawn_blocking(move || {
                let mut t = t;
                let items = t.read();
                (t, items)
            })
            .await?;
            let items = items?;
            if !items.is_empty() {
                tx.send(json!({"t": "chat", "session": session, "items": collapse(items)}))
                    .await?;
            }
            tail = Some((path, t));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn feed(agent: Agent, lines: &[Value]) -> Vec<Item> {
        let mut m = Mapper::new(agent, "/w/app");
        collapse(
            lines
                .iter()
                .flat_map(|l| m.line(l.to_string().as_bytes()))
                .collect(),
        )
    }

    fn kinds(items: &[Item]) -> Vec<String> {
        items
            .iter()
            .map(|i| match i {
                Item::User { text, .. } => format!("user {text}"),
                Item::Say { text, .. } => format!("say {text}"),
                Item::Tool {
                    tool,
                    summary,
                    status,
                    ..
                } => format!("{tool} {summary} {status:?}"),
                Item::Mark { text, .. } => format!("mark {text}"),
            })
            .collect()
    }

    #[test]
    fn a_claude_turn() {
        let items = feed(
            Agent::Claude,
            &[
                json!({"type":"user","uuid":"u1","message":{"content":"fix the test"}}),
                json!({"type":"user","uuid":"u0","isMeta":true,"message":{"content":"caveat"}}),
                json!({"type":"user","uuid":"u2","message":{"content":"<command-name>/model</command-name>"}}),
                json!({"type":"assistant","uuid":"a1","message":{"content":[{"type":"thinking","thinking":"hm"}]}}),
                json!({"type":"assistant","uuid":"a2","message":{"content":[{"type":"text","text":"Looking."}]}}),
                json!({"type":"assistant","uuid":"a3","message":{"content":[{"type":"tool_use","id":"t1","name":"Edit",
                    "input":{"file_path":"/w/app/src/x.rs","old_string":"a\nb\n","new_string":"a\nc\n"}}]}}),
                json!({"type":"assistant","uuid":"a4","message":{"content":[{"type":"tool_use","id":"t2","name":"Bash",
                    "input":{"command":"cargo  test\n -q"}}]}}),
                json!({"type":"user","uuid":"r1","message":{"content":[{"type":"tool_result","tool_use_id":"t1","content":"ok"}]}}),
                json!({"type":"user","uuid":"r2","message":{"content":[{"type":"tool_result","tool_use_id":"t2","is_error":true,
                    "content":"Permission for this command was denied by the auto mode classifier."}]}}),
                json!({"type":"assistant","uuid":"a5","isSidechain":true,"message":{"content":[{"type":"text","text":"sub"}]}}),
                json!({"type":"attachment","uuid":"q1","attachment":{"type":"queued_command","commandMode":"prompt","prompt":"also docs"}}),
                json!({"type":"system","subtype":"turn_duration","durationMs":125_400}),
                json!({"type":"user","uuid":"u3","message":{"content":[{"type":"text","text":"[Request interrupted by user]"}]}}),
            ],
        );
        assert_eq!(
            kinds(&items),
            [
                "user fix the test",
                "say Looking.",
                "Edit src/x.rs (+1/-1) Ok",
                "Bash cargo test -q Denied",
                "user also docs",
                "mark Worked for 2m 5s",
                "mark Interrupted",
            ]
        );
        let Item::Tool { detail, .. } = &items[2] else {
            panic!()
        };
        assert_eq!(detail.file.as_deref(), Some("src/x.rs"));
        assert!(detail.diff.as_deref().unwrap().contains("-b\n+c"));
        assert_eq!(detail.result.as_deref(), Some("ok"));
    }

    #[test]
    fn a_running_claude_tool_is_updated_in_place() {
        let mut m = Mapper::new(Agent::Claude, "");
        let first = m.line(
            json!({"type":"assistant","uuid":"a","message":{"content":[{"type":"tool_use","id":"t","name":"Read",
                "input":{"file_path":"/etc/hosts"}}]}})
            .to_string()
            .as_bytes(),
        );
        assert!(matches!(
            &first[..],
            [Item::Tool {
                status: Status::Running,
                ..
            }]
        ));
        let done = m.line(
            json!({"type":"user","uuid":"r","message":{"content":[{"type":"tool_result","tool_use_id":"t",
                "content":[{"type":"text","text":"127.0.0.1"}]}]}})
            .to_string()
            .as_bytes(),
        );
        assert_eq!(done[0].id(), "t");
        assert!(matches!(
            &done[..],
            [Item::Tool {
                status: Status::Ok,
                ..
            }]
        ));
        // A result for a call not seen (read from mid-file) is dropped.
        let orphan = m.line(
            json!({"type":"user","uuid":"r2","message":{"content":[{"type":"tool_result","tool_use_id":"zz","content":"x"}]}})
                .to_string()
                .as_bytes(),
        );
        assert!(orphan.is_empty());
    }

    #[test]
    fn a_codex_turn() {
        let ev = |item: Value| json!({"type":"event_msg","payload":{"type":"item_completed","item":item}});
        let items = feed(
            Agent::Codex,
            &[
                json!({"type":"response_item","payload":{"type":"message"}}),
                ev(
                    json!({"type":"UserMessage","id":"u","content":[{"type":"text","text":"touch a"}]}),
                ),
                ev(json!({"type":"Reasoning","id":"r"})),
                ev(
                    json!({"type":"AgentMessage","id":"m","content":[{"type":"Text","text":"On it."}]}),
                ),
                ev(
                    json!({"type":"CommandExecution","id":"c1","command":["/usr/bin/bash","-lc","touch a"],
                    "status":"completed","exit_code":0,"aggregated_output":""}),
                ),
                ev(
                    json!({"type":"CommandExecution","id":"c2","command":["/usr/bin/bash","-lc","false"],
                    "status":"completed","exit_code":1,"aggregated_output":"boom"}),
                ),
                ev(
                    json!({"type":"FileChange","id":"f","status":"completed","changes":{
                    "/w/app/a.txt":{"type":"update","unified_diff":"--- a\n+++ b\n@@\n-x\n+y\n+z"},
                    "/w/app/b.txt":{"type":"add","content":"1\n2\n"}}}),
                ),
                ev(
                    json!({"type":"Extension","kind":"web.search","id":"s","query":"rust  similar"}),
                ),
                ev(
                    json!({"type":"McpToolCall","id":"p","server":"node_repl","tool":"js",
                    "arguments":{"title":"Open the browser"},"status":"failed",
                    "result":{"content":[{"type":"text","text":"No browser"}],"isError":true}}),
                ),
                ev(json!({"type":"ImageView","id":"i","path":"file:///w/app/shot.png"})),
                json!({"type":"event_msg","payload":{"type":"task_complete","duration_ms":4000}}),
                json!({"type":"event_msg","payload":{"type":"turn_aborted","reason":"interrupted"}}),
            ],
        );
        assert_eq!(
            kinds(&items),
            [
                "user touch a",
                "say On it.",
                "Bash touch a Ok",
                "Bash false Error",
                "Edit a.txt (+2/-1) Ok",
                "Write b.txt (+2 lines) Ok",
                "WebSearch rust similar Ok",
                "node_repl.js Open the browser Error",
                "View shot.png Ok",
                "mark Worked for 4s",
                "mark Interrupted",
            ]
        );
        let Item::Tool { detail, .. } = &items[3] else {
            panic!()
        };
        assert_eq!(detail.result.as_deref(), Some("exit 1\nboom"));
    }

    #[test]
    fn a_tail_reads_whole_lines_as_they_arrive() {
        use std::io::Write;
        let dir = std::env::temp_dir().join(format!("valk-tail-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("c.jsonl");
        let line = |uuid: &str, text: &str| {
            json!({"type":"user","uuid":uuid,"message":{"content":text}}).to_string()
        };
        std::fs::write(&path, format!("{}\nnot json\n", line("1", "one"))).unwrap();
        let mut tail = Tail::open(&path, "").unwrap();
        assert_eq!(kinds(&tail.read_all().unwrap()), ["user one"]);
        let mut f = std::fs::OpenOptions::new()
            .append(true)
            .open(&path)
            .unwrap();
        let two = line("2", "two");
        let (a, b) = two.split_at(10);
        write!(f, "{a}").unwrap();
        assert!(tail.read().unwrap().is_empty(), "half a line waits");
        writeln!(f, "{b}").unwrap();
        assert_eq!(kinds(&tail.read().unwrap()), ["user two"]);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn items_serialize_for_the_app() {
        let item = Item::Tool {
            id: "t".into(),
            tool: "Bash".into(),
            summary: "ls".into(),
            status: Status::Denied,
            detail: Detail {
                command: Some("ls".into()),
                ..Detail::default()
            },
        };
        assert_eq!(
            serde_json::to_value(&item).unwrap(),
            json!({"k":"tool","id":"t","tool":"Bash","summary":"ls","status":"denied",
                   "detail":{"command":"ls"}})
        );
    }
}
