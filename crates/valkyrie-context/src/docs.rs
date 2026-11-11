//! Docs (ADR-0021): markdown written for a person, kept in a folder the user picks,
//! usually inside an Obsidian vault. Agent-level docs sit at the root, each
//! repository's under `repos/<name>/`. Agents and the user edit the same files, so
//! an edit must start from what's on disk, and every version it replaces is kept.

use crate::{project, shown, write_atomic};
use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use valkyrie_proto::{Doc, DocKind, NewDoc};

/// Longest title, in characters.
const MAX_TITLE: usize = 120;
/// Largest body, in bytes.
pub const MAX_BODY: usize = 200 * 1024;
/// Versions kept per doc, and for how long.
const HISTORY_KEEP: usize = 50;
const HISTORY_DAYS: u64 = 30;
/// A repository's folder name, before any hash.
const MAX_FOLDER: usize = 40;
/// How much of a file listing reads for its front matter.
const MAX_HEAD: u64 = 64 * 1024;
/// The largest file the web app is sent: a vault can hold anything.
pub const MAX_OPEN: u64 = 2 * 1024 * 1024;
/// How long an unread count stands before the folder is walked again. Writes
/// through the daemon clear it at once; this only bounds a file an agent wrote
/// to disk itself.
const COUNT_FOR: std::time::Duration = std::time::Duration::from_secs(30);

/// Who wrote or edited a doc, as the daemon worked it out (never the client).
#[derive(Debug, Clone, Default)]
pub struct Author {
    /// The agent, or `human`.
    pub by: String,
    pub session: Option<String>,
    pub conversation: Option<String>,
    pub commit: Option<String>,
    pub item: Option<u32>,
}

/// What the daemon keeps about docs in `<state dir>/docs.json`.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
struct Config {
    /// The folder the user picked; `None`: `<state dir>/docs`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    root: Option<PathBuf>,
    /// Project key (`project::key`) to its folder's name under `repos/`, chosen once.
    #[serde(default)]
    folders: BTreeMap<String, String>,
    /// The next `valk-id` to give out.
    #[serde(default)]
    next_id: u32,
    /// When the user last opened the Docs list (ADR-0021 §5), in seconds.
    #[serde(default)]
    seen: u64,
}

pub struct Docs {
    state: PathBuf,
    config: Config,
    /// Each file's front matter as last read, by path, while its mtime and size
    /// hold (ADR-0021 §5): listing and finding by id stat files instead of
    /// reading them.
    cache: BTreeMap<PathBuf, Cached>,
    /// The last unread count and when it was taken, for the Inbox's poll.
    count: Option<(std::time::Instant, u32)>,
}

#[derive(Clone)]
struct Cached {
    /// mtime in nanoseconds, and size.
    stamp: (u128, u64),
    front: Front,
}

impl Docs {
    /// Docs kept under `state` (the daemon's state dir).
    pub fn new(state: PathBuf) -> Self {
        let config = std::fs::read_to_string(state.join("docs.json"))
            .ok()
            .and_then(|text| serde_json::from_str(&text).ok())
            .unwrap_or_default();
        Self {
            state,
            config,
            cache: BTreeMap::new(),
            count: None,
        }
    }

    fn save(&self) -> Result<()> {
        valkyrie_proto::private_dir_all(&self.state)?;
        write_atomic(
            &self.state.join("docs.json"),
            &serde_json::to_string_pretty(&self.config)?,
        )
    }

    /// Where docs live now.
    pub fn root(&self) -> PathBuf {
        self.config
            .root
            .clone()
            .unwrap_or_else(|| self.state.join("docs"))
    }

    /// Whether the user picked the root, rather than it being the default.
    pub fn root_is_set(&self) -> bool {
        self.config.root.is_some()
    }

    /// Points docs at `root` (made if missing), or back at the default with `None`.
    /// Moves nothing: docs already written stay where they are. The caller checks
    /// that the user asked (ADR-0021 §1).
    pub fn set_root(&mut self, root: Option<&Path>) -> Result<PathBuf> {
        let root = match root {
            None => None,
            Some(root) => {
                if !root.is_absolute() {
                    bail!("give the docs folder as an absolute path");
                }
                std::fs::create_dir_all(root)
                    .with_context(|| format!("make {}", root.display()))?;
                let real = valkyrie_proto::canonical(root)?;
                let home = valkyrie_proto::canonical(&valkyrie_proto::home_dir()).ok();
                if Some(&real) == home.as_ref() || real.parent().is_none() {
                    bail!(
                        "{} is too wide: pick a folder for agent docs, like ~/vault/valk",
                        real.display()
                    );
                }
                Some(real)
            }
        };
        self.config.root = root;
        self.cache.clear();
        self.count = None;
        self.save()?;
        Ok(self.root())
    }

    /// The canonical root, made if missing.
    fn real_root(&self) -> Result<PathBuf> {
        let root = self.root();
        if self.config.root.is_none() {
            valkyrie_proto::private_dir_all(&root)?;
        } else {
            std::fs::create_dir_all(&root)?;
        }
        Ok(valkyrie_proto::canonical(&root)?)
    }

    /// The folder for docs about `repo` (a project root), or the root for
    /// agent-level ones. A name another repository already has gets its project
    /// hash (ADR-0021 §2), decided once.
    fn folder(&mut self, root: &Path, repo: Option<&Path>) -> Result<PathBuf> {
        let Some(repo) = repo else {
            return Ok(root.to_path_buf());
        };
        let key = project::key(repo);
        let name = match self.config.folders.get(&key) {
            Some(name) => name.clone(),
            None => {
                let base = folder_name(repo);
                let taken = self.config.folders.values().any(|n| *n == base);
                let name = match taken {
                    // `key` ends in the project's 8-hex hash.
                    true => format!("{base}-{}", &key[key.len() - 8..key.len() - 2]),
                    false => base,
                };
                self.config.folders.insert(key, name.clone());
                self.save()?;
                name
            }
        };
        let dir = root.join("repos").join(name);
        std::fs::create_dir_all(&dir)?;
        inside(root, &dir)?;
        Ok(dir)
    }

    /// Writes a new doc and returns it, with its text and hash.
    pub fn write(&mut self, new: &NewDoc, author: &Author, now: u64) -> Result<Doc> {
        let title = clean_title(&new.title)?;
        let body = clean_body(&new.body)?;
        let root = self.real_root()?;
        let repo = (!new.global && project::in_git(&new.cwd)).then(|| project::root(&new.cwd));
        let dir = self.folder(&root, repo.as_deref())?;
        let id = self.take_id(&root)?;
        let front = Front {
            id,
            kind: new.kind,
            title: title.clone(),
            created: rfc3339(now),
            by: author.by.clone(),
            session: author.session.clone(),
            conversation: author.conversation.clone(),
            commit: author.commit.clone(),
            item: author.item,
            repo: repo.as_ref().map(|r| r.display().to_string()),
            edited: Vec::new(),
            rest: Vec::new(),
        };
        let text = front.render(&body);
        let stem = format!("{}-{}", &rfc3339(now)[..10], slug(&title));
        let path = create_new(&dir, &stem, &text)?;
        self.count = None;
        Ok(doc(&root, &path, &front, text))
    }

    /// One past the highest id given out or found on disk, so an id is never
    /// given twice even if `docs.json` was lost.
    fn take_id(&mut self, root: &Path) -> Result<u32> {
        let on_disk = self
            .index(root)
            .iter()
            .filter_map(|(_, c)| c.front.id_opt())
            .max()
            .unwrap_or(0);
        let id = self.config.next_id.max(on_disk + 1).max(1);
        self.config.next_id = id + 1;
        self.save()?;
        Ok(id)
    }

    /// Every doc under the root, newest first. Files without a `valk-id` (the
    /// user's own) are listed with id 0 and a handle for a key.
    pub fn list(&mut self) -> Result<Vec<Doc>> {
        let root = self.real_root()?;
        let seen = self.config.seen;
        // Front matter is the writer's word: a date in the future can't pin a doc
        // unread past its file's own mtime. Unread is a nudge, not a guarantee: an
        // agent writing the file itself can say `by: human`.
        let mut docs: Vec<(u128, Doc)> = self
            .index(&root)
            .into_iter()
            .map(|(path, c)| {
                let mut d = doc(&root, &path, &c.front, String::new());
                d.key = key(&c.front, &d.path);
                let mtime = u64::try_from(c.stamp.0 / 1_000_000_000).unwrap_or(u64::MAX);
                d.unread = c.front.agent_at().min(mtime) > seen;
                (c.stamp.0, d)
            })
            .collect();
        docs.sort_by(|a, b| b.0.cmp(&a.0).then_with(|| a.1.path.cmp(&b.1.path)));
        let docs: Vec<Doc> = docs.into_iter().map(|(_, d)| d).collect();
        self.count = Some((std::time::Instant::now(), Self::unread(&docs)));
        Ok(docs)
    }

    /// How many docs are unread, for a poll: the last count while it's fresh
    /// and nothing was written through the daemon since, so polling devices
    /// don't each walk the whole folder.
    pub fn unread_count(&mut self) -> Result<u32> {
        match self.count {
            Some((at, n)) if at.elapsed() < COUNT_FOR => Ok(n),
            _ => Ok(Self::unread(&self.list()?)),
        }
    }

    /// How many docs an agent wrote or edited since the list was last opened.
    pub fn unread(docs: &[Doc]) -> u32 {
        docs.iter().filter(|d| d.unread).count() as u32
    }

    /// The user opened the Docs list as it was at `at`: what agents wrote until
    /// then is read, not what came in while it loaded.
    pub fn seen(&mut self, at: u64, now: u64) -> Result<()> {
        self.count = None;
        self.config.seen = self.config.seen.max(at.min(now));
        self.save()
    }

    /// Every `.md` file under `root` with its front matter, read again only for a
    /// file whose mtime or size changed since the last call.
    fn index(&mut self, root: &Path) -> Vec<(PathBuf, Cached)> {
        let mut fresh = BTreeMap::new();
        for path in scan(root) {
            let Some(stamp) = stamp(&path) else { continue };
            let cached = match self.cache.remove(&path) {
                Some(c) if c.stamp == stamp => c,
                _ => {
                    let Some(head) = read_head(&path) else {
                        continue;
                    };
                    let front = Front::parse(&head).0.unwrap_or_else(|| Front::bare(&path));
                    Cached { stamp, front }
                }
            };
            fresh.insert(path, cached);
        }
        self.cache = fresh;
        self.cache
            .iter()
            .map(|(p, c)| (p.clone(), c.clone()))
            .collect()
    }

    /// The web app's read (ADR-0021 §5): a doc by the key a list gave out, its
    /// `valk-id` or the handle of one of the user's own. Never a path, and never
    /// a write: a doc with no id is sent as it is.
    pub fn open(&mut self, key_: &str) -> Result<Doc> {
        let root = self.real_root()?;
        let found = self.index(&root).into_iter().find(|(path, c)| {
            let rel = slashed(path.strip_prefix(&root).unwrap_or(path));
            !key_.is_empty() && key(&c.front, &rel) == key_
        });
        let Some((path, _)) = found else {
            bail!("no doc {key_} under {}", root.display());
        };
        // Between the scan and now the file could have become a symlink, or grown.
        if !std::fs::symlink_metadata(&path)?.is_file() {
            bail!("doc {key_} is no longer a file");
        }
        inside(&root, &path)?;
        let mut bytes = Vec::new();
        std::io::Read::read_to_end(
            &mut std::io::Read::take(std::fs::File::open(&path)?, MAX_OPEN + 1),
            &mut bytes,
        )?;
        if bytes.len() as u64 > MAX_OPEN {
            bail!(
                "doc {key_} is over {} MB: open it in your editor",
                MAX_OPEN >> 20
            );
        }
        let text = String::from_utf8(bytes).context("the doc isn't UTF-8 text")?;
        let front = Front::parse(&text).0.unwrap_or_else(|| Front::bare(&path));
        let mut d = doc(&root, &path, &front, text);
        d.key = key(&front, &d.path);
        Ok(d)
    }

    /// A doc by its `valk-id`, or by its path under the root. A file without an id
    /// (one the user made in Obsidian) gets one now, so it can be edited by id.
    pub fn read(&mut self, which: &str) -> Result<Doc> {
        let root = self.real_root()?;
        let path = self.find(&root, which)?;
        let text = std::fs::read_to_string(&path)?;
        match Front::parse(&text) {
            (Some(front), _) if front.id_opt().is_some() => Ok(doc(&root, &path, &front, text)),
            (front, body) => {
                let mut front = front.unwrap_or_else(|| Front::bare(&path));
                front.id = self.take_id(&root)?;
                if front.by.is_empty() {
                    front.by = "human".into();
                }
                if front.title.is_empty() {
                    front.title = Front::bare(&path).title;
                }
                let text = front.render(body);
                write_atomic(&path, &text)?;
                self.count = None;
                Ok(doc(&root, &path, &front, text))
            }
        }
    }

    fn find(&mut self, root: &Path, which: &str) -> Result<PathBuf> {
        if let Ok(id) = which.trim_start_matches('#').parse::<u32>() {
            return self
                .index(root)
                .into_iter()
                .find(|(_, c)| c.front.id_opt() == Some(id))
                .map(|(p, _)| p)
                .with_context(|| format!("no doc {id} under {}", root.display()));
        }
        let path = root.join(which);
        if path.extension().is_none_or(|e| e != "md") {
            bail!("a doc is a .md file under {}", root.display());
        }
        let real = valkyrie_proto::canonical(&path)
            .with_context(|| format!("no doc {which} under {}", root.display()))?;
        inside(root, &real)?;
        Ok(real)
    }

    /// Replaces a doc's body with `body`, if `base` is the hash of what's on disk
    /// (ADR-0021 §4). The version it replaces goes into the doc's history.
    pub fn edit(
        &mut self,
        id: u32,
        base: &str,
        body: &str,
        author: &Author,
        now: u64,
    ) -> Result<Doc> {
        let body = clean_body(body)?;
        let root = self.real_root()?;
        let path = self.find(&root, &id.to_string())?;
        let old = std::fs::read_to_string(&path)?;
        if hash(&old) != base.trim() {
            bail!(
                "doc {id} changed since you read it (now {}): read it again with `valk doc read {id}`, \
                 fold your change into what's there, and edit with the new hash",
                hash(&old)
            );
        }
        let (Some(mut front), _) = Front::parse(&old) else {
            bail!("doc {id} has lost its front matter: read it again");
        };
        self.keep(id, &old, now)?;
        front.edited.push(match &author.session {
            Some(session) => format!("{}, {session}, {}", author.by, rfc3339(now)),
            None => format!("{}, {}", author.by, rfc3339(now)),
        });
        let text = front.render(&body);
        write_atomic(&path, &text)?;
        self.count = None;
        Ok(doc(&root, &path, &front, text))
    }

    /// Saves `text` as one of `id`'s past versions, then trims the history.
    fn keep(&self, id: u32, text: &str, now: u64) -> Result<()> {
        let dir = self.state.join("docs-history").join(id.to_string());
        valkyrie_proto::private_dir_all(&dir)?;
        let mut name = format!("{now}.md");
        for n in 2.. {
            if !dir.join(&name).exists() {
                break;
            }
            name = format!("{now}-{n}.md");
        }
        write_atomic(&dir.join(name), text)?;
        let mut kept: Vec<(u64, PathBuf)> = std::fs::read_dir(&dir)?
            .flatten()
            .filter_map(|e| {
                let at = e
                    .file_name()
                    .to_str()?
                    .split(['.', '-'])
                    .next()?
                    .parse()
                    .ok()?;
                Some((at, e.path()))
            })
            .collect();
        kept.sort_by_key(|k| std::cmp::Reverse(k.0));
        for (i, (at, path)) in kept.iter().enumerate() {
            let old = now.saturating_sub(*at) > HISTORY_DAYS * 86_400;
            if i >= HISTORY_KEEP || (i > 0 && old) {
                let _ = std::fs::remove_file(path);
            }
        }
        Ok(())
    }

    /// `id`'s past versions, newest first: when each was replaced, and its path.
    pub fn history(&self, id: u32) -> Vec<(u64, PathBuf)> {
        let dir = self.state.join("docs-history").join(id.to_string());
        let mut out: Vec<(u64, PathBuf)> = std::fs::read_dir(dir)
            .into_iter()
            .flatten()
            .flatten()
            .filter_map(|e| {
                let at = e
                    .file_name()
                    .to_str()?
                    .split(['.', '-'])
                    .next()?
                    .parse()
                    .ok()?;
                Some((at, e.path()))
            })
            .collect();
        out.sort_by_key(|k| std::cmp::Reverse(k.0));
        out
    }
}

/// The front matter Valkyrie writes, in YAML so Obsidian shows it as properties.
#[derive(Debug, Clone, Default)]
struct Front {
    id: u32,
    kind: DocKind,
    title: String,
    created: String,
    by: String,
    session: Option<String>,
    conversation: Option<String>,
    commit: Option<String>,
    item: Option<u32>,
    repo: Option<String>,
    edited: Vec<String>,
    /// Lines Valkyrie doesn't know (the user's own properties), kept as they were.
    rest: Vec<String>,
}

impl Front {
    fn id_opt(&self) -> Option<u32> {
        (self.id > 0).then_some(self.id)
    }

    /// When an agent last wrote or edited it, in seconds; 0 if only people have.
    /// What makes a doc unread: the user's own saves in Obsidian don't.
    fn agent_at(&self) -> u64 {
        let agent = |by: &str| !by.is_empty() && by != "human";
        let wrote = agent(&self.by)
            .then(|| parse_rfc3339(&self.created))
            .flatten();
        // Each edit is "by, session, when" or "by, when".
        let edits = self.edited.iter().filter_map(|e| {
            let by = e.split(',').next()?.trim();
            agent(by).then(|| parse_rfc3339(e.rsplit(',').next()?))?
        });
        wrote.into_iter().chain(edits).max().unwrap_or(0)
    }

    /// For a file with no front matter: its name is its title.
    fn bare(path: &Path) -> Front {
        Front {
            title: path
                .file_stem()
                .map(|s| s.to_string_lossy().into_owned())
                .unwrap_or_default(),
            ..Front::default()
        }
    }

    fn render(&self, body: &str) -> String {
        let q = |s: &str| serde_json::to_string(s).unwrap_or_default();
        let mut out = String::from("---\n");
        out += &format!("valk-id: {}\n", self.id);
        out += &format!("kind: {}\n", self.kind.as_str());
        out += &format!("title: {}\n", q(&self.title));
        if !self.created.is_empty() {
            out += &format!("created: {}\n", self.created);
        }
        if !self.by.is_empty() {
            out += &format!("by: {}\n", q(&self.by));
        }
        for (key, value) in [
            ("session", &self.session),
            ("conversation", &self.conversation),
            ("commit", &self.commit),
            ("repo", &self.repo),
        ] {
            if let Some(v) = value {
                out += &format!("{key}: {}\n", q(v));
            }
        }
        if let Some(item) = self.item {
            out += &format!("item: {item}\n");
        }
        if !self.edited.is_empty() {
            out += "edited:\n";
            for e in &self.edited {
                out += &format!("  - {}\n", q(e));
            }
        }
        for line in &self.rest {
            out += line;
            out.push('\n');
        }
        out += "---\n\n";
        out += body.trim_end();
        out.push('\n');
        out
    }

    /// The front matter, if the text opens with one, and the body after it.
    fn parse(text: &str) -> (Option<Front>, &str) {
        let Some(after) = text.strip_prefix("---\n") else {
            return (None, text);
        };
        let Some(end) = after
            .find("\n---\n")
            .map(|i| (i, i + 5))
            .or_else(|| after.strip_suffix("\n---").map(|s| (s.len(), after.len())))
        else {
            return (None, text);
        };
        let (head, body) = (&after[..end.0], &after[end.1..]);
        let unq = |v: &str| {
            let v = v.trim();
            serde_json::from_str::<String>(v)
                .unwrap_or_else(|_| v.trim_matches(['\'', '"']).to_owned())
        };
        let mut front = Front::default();
        let mut in_edited = false;
        for line in head.lines() {
            if in_edited && let Some(entry) = line.trim_start().strip_prefix("- ") {
                front.edited.push(unq(entry));
                continue;
            }
            in_edited = false;
            let Some((key, value)) = line.split_once(':').filter(|_| !line.starts_with(' ')) else {
                front.rest.push(line.to_owned());
                continue;
            };
            let opt = |v: &str| Some(unq(v)).filter(|v| !v.is_empty());
            match key.trim() {
                "valk-id" => front.id = value.trim().parse().unwrap_or(0),
                "kind" => front.kind = DocKind::parse(&unq(value)).unwrap_or_default(),
                "title" => front.title = unq(value),
                "created" => front.created = unq(value),
                "by" => front.by = unq(value),
                "session" => front.session = opt(value),
                "conversation" => front.conversation = opt(value),
                "commit" => front.commit = opt(value),
                "repo" => front.repo = opt(value),
                "item" => front.item = value.trim().parse().ok(),
                "edited" => in_edited = true,
                _ => front.rest.push(line.to_owned()),
            }
        }
        (Some(front), body.strip_prefix('\n').unwrap_or(body))
    }
}

fn doc(root: &Path, path: &Path, front: &Front, text: String) -> Doc {
    Doc {
        id: front.id,
        kind: front.kind,
        title: front.title.clone(),
        path: slashed(path.strip_prefix(root).unwrap_or(path)),
        key: String::new(),
        unread: false,
        by: front.by.clone(),
        created: front.created.clone(),
        session: front.session.clone(),
        commit: front.commit.clone(),
        item: front.item,
        repo: front.repo.clone(),
        edited: front.edited.clone(),
        hash: (!text.is_empty()).then(|| hash(&text)),
        text: (!text.is_empty()).then_some(text),
    }
}

fn clean_title(title: &str) -> Result<String> {
    let title = crate::file::one_line(&shown(title, false));
    if title.is_empty() {
        bail!("a doc needs a title");
    }
    if title.chars().count() > MAX_TITLE {
        bail!("title is over {MAX_TITLE} characters");
    }
    Ok(title)
}

fn clean_body(body: &str) -> Result<String> {
    let body = shown(body, true);
    if body.len() > MAX_BODY {
        bail!("doc is over {} KB", MAX_BODY / 1024);
    }
    if body.trim().is_empty() {
        bail!("a doc needs some text");
    }
    Ok(body)
}

/// A file name from a title: lowercase words joined by `-`, at most 60 characters.
fn slug(title: &str) -> String {
    let mut out = String::new();
    for c in title.chars().flat_map(char::to_lowercase) {
        if c.is_alphanumeric() {
            out.push(c);
        } else if !out.ends_with('-') && !out.is_empty() {
            out.push('-');
        }
        if out.chars().count() >= 60 {
            break;
        }
    }
    let out = out.trim_end_matches('-').to_owned();
    if out.is_empty() { "doc".into() } else { out }
}

/// A repository's folder name: its directory's, made safe.
fn folder_name(repo: &Path) -> String {
    let name: String = repo
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default()
        .chars()
        .map(|c| match c {
            'a'..='z' | 'A'..='Z' | '0'..='9' | '.' | '-' | '_' => c,
            _ => '_',
        })
        .take(MAX_FOLDER)
        .collect();
    match name.trim_start_matches('.') {
        "" => "repo".into(),
        name => name.to_owned(),
    }
}

/// Creates `<stem>.md` in `dir`, or `<stem>-2.md` and so on, never over a file.
fn create_new(dir: &Path, stem: &str, text: &str) -> Result<PathBuf> {
    use std::io::Write;
    for n in 1..1000 {
        let name = match n {
            1 => format!("{stem}.md"),
            n => format!("{stem}-{n}.md"),
        };
        let path = dir.join(name);
        match std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&path)
        {
            Ok(mut file) => {
                file.write_all(text.as_bytes())?;
                return Ok(path);
            }
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(e) => return Err(e).with_context(|| format!("write {}", path.display())),
        }
    }
    bail!("too many docs named {stem} in {}", dir.display())
}

/// A path under the root as the web app and the CLI name it: `/` between
/// folders on every platform.
fn slashed(path: &Path) -> String {
    path.components()
        .map(|c| c.as_os_str().to_string_lossy())
        .collect::<Vec<_>>()
        .join("/")
}

/// Refuses `path` unless it resolves inside `root`: a symlinked folder can't take
/// a write, or the web app's read, outside it.
fn inside(root: &Path, path: &Path) -> Result<()> {
    let real = valkyrie_proto::canonical(path)?;
    if !real.starts_with(root) {
        bail!("{} is outside the docs folder", path.display());
    }
    Ok(())
}

/// Every `.md` file under `root`, not following symlinks, skipping dotted folders
/// (`.obsidian`, `.trash`).
fn scan(root: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    let mut dirs = vec![root.to_path_buf()];
    while let Some(dir) = dirs.pop() {
        for entry in std::fs::read_dir(&dir).into_iter().flatten().flatten() {
            let name = entry.file_name();
            if name.to_string_lossy().starts_with('.') {
                continue;
            }
            let Ok(kind) = entry.file_type() else {
                continue;
            };
            let path = entry.path();
            if kind.is_dir() {
                dirs.push(path);
            } else if kind.is_file() && path.extension().is_some_and(|e| e == "md") {
                out.push(path);
            }
        }
    }
    out.sort();
    out
}

/// What the web app opens a doc by: its id, or `p` and its path's hash for one with
/// none. The daemon matches it against its own scan, so a client never names a file.
fn key(front: &Front, rel: &str) -> String {
    match front.id_opt() {
        Some(id) => id.to_string(),
        None => format!("p{}", hash(rel)),
    }
}

/// A file's mtime (ns) and size, if it's still a plain file.
fn stamp(path: &Path) -> Option<(u128, u64)> {
    let meta = std::fs::symlink_metadata(path).ok()?;
    if !meta.is_file() {
        return None;
    }
    let mtime = meta
        .modified()
        .ok()?
        .duration_since(std::time::UNIX_EPOCH)
        .ok()?
        .as_nanos();
    Some((mtime, meta.len()))
}

/// The start of a file, enough for its front matter.
fn read_head(path: &Path) -> Option<String> {
    use std::io::Read;
    let mut buf = Vec::new();
    std::fs::File::open(path)
        .ok()?
        .take(MAX_HEAD)
        .read_to_end(&mut buf)
        .ok()?;
    Some(String::from_utf8_lossy(&buf).into_owned())
}

/// A file's hash for edits (ADR-0021 §4): it tells versions apart, not secrets.
pub fn hash(text: &str) -> String {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for b in text.as_bytes() {
        h ^= u64::from(*b);
        h = h.wrapping_mul(0x0100_0000_01b3);
    }
    format!("{h:016x}")
}

/// `2026-11-09T21:04:05Z`.
pub fn rfc3339(secs: u64) -> String {
    let days = (secs / 86_400) as i64;
    let rem = secs % 86_400;
    // Civil from days (Howard Hinnant's algorithm).
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = yoe + era * 400 + i64::from(m <= 2);
    format!(
        "{y:04}-{m:02}-{d:02}T{:02}:{:02}:{:02}Z",
        rem / 3600,
        rem % 3600 / 60,
        rem % 60
    )
}

/// Seconds from `2026-11-09T21:04:05Z`, the form `rfc3339` writes.
fn parse_rfc3339(s: &str) -> Option<u64> {
    let s = s.trim().strip_suffix('Z')?;
    let (date, time) = s.split_once('T')?;
    let mut d = date.splitn(3, '-').map(|p| p.parse::<i64>().ok());
    let (y, m, d) = (d.next()??, d.next()??, d.next()??);
    let mut t = time.splitn(3, ':').map(|p| p.parse::<u64>().ok());
    let (hh, mm, ss) = (t.next()??, t.next()??, t.next()??);
    if !(1..=12).contains(&m) || !(1..=31).contains(&d) || hh > 23 || mm > 59 || ss > 60 {
        return None;
    }
    // Days from civil (Howard Hinnant's algorithm).
    let y = if m <= 2 { y - 1 } else { y };
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let mp = if m > 2 { m - 3 } else { m + 9 };
    let doy = (153 * mp + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    let days = u64::try_from(era * 146_097 + doe - 719_468).ok()?;
    Some(days * 86_400 + hh * 3600 + mm * 60 + ss)
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Fixture {
        base: PathBuf,
        docs: Docs,
        repo: PathBuf,
    }

    impl Fixture {
        fn new(name: &str) -> Self {
            let base =
                std::env::temp_dir().join(format!("valk-docs-{name}-{}", std::process::id()));
            let _ = std::fs::remove_dir_all(&base);
            let repo = base.join("code/valkyrie");
            std::fs::create_dir_all(repo.join(".git")).unwrap();
            std::fs::write(repo.join(".git/HEAD"), "ref: refs/heads/main\n").unwrap();
            let repo = valkyrie_proto::canonical(&repo).unwrap();
            let docs = Docs::new(base.join("state"));
            Self { base, docs, repo }
        }

        fn new_doc(&self, title: &str, global: bool) -> NewDoc {
            NewDoc {
                cwd: self.repo.clone(),
                title: title.into(),
                kind: DocKind::Plan,
                global,
                body: "# Plan\n\n1. one\n2. two\n".into(),
            }
        }
    }

    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.base);
        }
    }

    fn claude() -> Author {
        Author {
            by: "claude".into(),
            session: Some("#4 docs".into()),
            conversation: Some("abc-123".into()),
            commit: Some("1a2b3c4".into()),
            item: Some(132),
        }
    }

    const NOV_9: u64 = 1_794_258_000; // 2026-11-09T21:00:00Z

    #[test]
    fn dates_read_as_rfc3339() {
        assert_eq!(rfc3339(NOV_9), "2026-11-09T21:00:00Z");
        assert_eq!(rfc3339(0), "1970-01-01T00:00:00Z");
        assert_eq!(rfc3339(951_782_400), "2000-02-29T00:00:00Z");
        for secs in [0, NOV_9, 951_782_400, 4_102_444_799] {
            assert_eq!(parse_rfc3339(&rfc3339(secs)), Some(secs));
        }
        assert_eq!(parse_rfc3339("2026-13-01T00:00:00Z"), None);
        assert_eq!(parse_rfc3339("yesterday"), None);
    }

    #[test]
    fn the_web_app_opens_by_key_and_writes_nothing() {
        let mut f = Fixture::new("open");
        let root = f.docs.real_root().unwrap();
        let mine = "---\ntags: [homelab]\n---\nmy notes\n";
        std::fs::write(root.join("notes.md"), mine).unwrap();
        let theirs = f
            .docs
            .write(&f.new_doc("Plan", true), &claude(), NOV_9)
            .unwrap();

        let listed = f.docs.list().unwrap();
        let own = listed.iter().find(|d| d.path == "notes.md").unwrap();
        assert_eq!(own.id, 0);
        assert!(own.key.starts_with('p'), "{own:?}");
        let agents = listed.iter().find(|d| d.id == theirs.id).unwrap();
        assert_eq!(agents.key, theirs.id.to_string());

        // The user's own doc is sent as it is: no id written into it.
        let opened = f.docs.open(&own.key).unwrap();
        assert_eq!(opened.text.as_deref(), Some(mine));
        assert_eq!(opened.id, 0);
        assert_eq!(
            std::fs::read_to_string(root.join("notes.md")).unwrap(),
            mine
        );
        assert_eq!(f.docs.open(&agents.key).unwrap().title, "Plan");

        // Never by path, nor by anything a list didn't give out.
        for bad in [
            "notes.md",
            "../secret.md",
            "/etc/passwd",
            "",
            "p",
            "#1",
            "p0000000000000000",
        ] {
            assert!(f.docs.open(bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn the_web_app_opens_nothing_outside_the_root_nor_too_big() {
        let mut f = Fixture::new("open-outside");
        let root = f.docs.real_root().unwrap();
        std::fs::write(f.base.join("secret.md"), "---\nvalk-id: 99\n---\nno").unwrap();
        #[cfg(unix)]
        {
            std::os::unix::fs::symlink(f.base.join("secret.md"), root.join("link.md")).unwrap();
            std::os::unix::fs::symlink(&f.base, root.join("up")).unwrap();
            assert!(f.docs.open("99").is_err());
        }
        std::fs::create_dir_all(root.join(".obsidian")).unwrap();
        std::fs::write(root.join(".obsidian/hidden.md"), "---\nvalk-id: 98\n---\n").unwrap();
        assert!(f.docs.open("98").is_err());
        let big = format!("---\nvalk-id: 97\n---\n{}", "x".repeat(MAX_OPEN as usize));
        std::fs::write(root.join("big.md"), big).unwrap();
        assert!(f.docs.list().unwrap().iter().any(|d| d.id == 97));
        let err = f.docs.open("97").unwrap_err().to_string();
        assert!(err.contains("over 2 MB"), "{err}");
    }

    #[test]
    fn the_unread_count_stands_until_the_daemon_writes() {
        let mut f = Fixture::new("count");
        let root = f.docs.real_root().unwrap();
        f.docs
            .write(&f.new_doc("One", true), &claude(), NOV_9)
            .unwrap();
        assert_eq!(f.docs.unread_count().unwrap(), 1);
        // Written behind the daemon's back: the count stands until it's stale.
        std::fs::write(
            root.join("sneaky.md"),
            "---\nvalk-id: 40\nby: codex\ncreated: 2026-11-09T22:00:00Z\n---\n",
        )
        .unwrap();
        assert_eq!(f.docs.unread_count().unwrap(), 1);
        // As if it were taken long ago (or dropped, on a clock too young for that).
        f.docs.count = f.docs.count.and_then(|(at, n)| {
            at.checked_sub(COUNT_FOR + std::time::Duration::from_secs(1))
                .map(|at| (at, n))
        });
        assert_eq!(f.docs.unread_count().unwrap(), 2);
        // A write through the daemon, or the list being opened, counts again now.
        f.docs
            .write(&f.new_doc("Two", true), &claude(), NOV_9 + 5)
            .unwrap();
        assert_eq!(f.docs.unread_count().unwrap(), 3);
        f.docs.seen(u64::MAX, NOV_9 + 1_000_000_000).unwrap();
        assert_eq!(f.docs.unread_count().unwrap(), 0);
    }

    #[test]
    fn the_list_rereads_a_file_only_when_it_changes() {
        let mut f = Fixture::new("cache");
        let root = f.docs.real_root().unwrap();
        let path = root.join("a.md");
        std::fs::write(&path, "---\ntitle: one\n---\n").unwrap();
        assert_eq!(f.docs.list().unwrap()[0].title, "one");
        // Same mtime and size: the cached front matter stands.
        let mtime = std::fs::metadata(&path).unwrap().modified().unwrap();
        std::fs::write(&path, "---\ntitle: two\n---\n").unwrap();
        std::fs::File::options()
            .write(true)
            .open(&path)
            .unwrap()
            .set_modified(mtime)
            .unwrap();
        assert_eq!(f.docs.list().unwrap()[0].title, "one");
        // A new mtime: read again.
        std::fs::File::options()
            .write(true)
            .open(&path)
            .unwrap()
            .set_modified(mtime + std::time::Duration::from_secs(5))
            .unwrap();
        assert_eq!(f.docs.list().unwrap()[0].title, "two");
        // Gone from disk, gone from the list.
        std::fs::remove_file(&path).unwrap();
        assert!(f.docs.list().unwrap().is_empty());
    }

    #[test]
    fn a_doc_an_agent_wrote_or_edited_since_the_list_was_opened_is_unread() {
        let mut f = Fixture::new("unread");
        let root = f.docs.real_root().unwrap();
        let human = Author {
            by: "human".into(),
            ..Author::default()
        };
        let old = f
            .docs
            .write(&f.new_doc("Old", true), &claude(), NOV_9)
            .unwrap();
        f.docs
            .write(&f.new_doc("Mine", true), &human, NOV_9 + 20)
            .unwrap();
        std::fs::write(root.join("obsidian.md"), "my own\n").unwrap();
        let unread = |docs: &mut Docs| {
            let mut t: Vec<String> = docs
                .list()
                .unwrap()
                .into_iter()
                .filter(|d| d.unread)
                .map(|d| d.title)
                .collect();
            t.sort();
            t
        };
        assert_eq!(unread(&mut f.docs), ["Old"]);
        f.docs.seen(NOV_9 + 10, NOV_9 + 10).unwrap();
        assert!(unread(&mut f.docs).is_empty());

        let new = f
            .docs
            .write(&f.new_doc("New", true), &claude(), NOV_9 + 30)
            .unwrap();
        assert_eq!(unread(&mut f.docs), ["New"]);
        assert_eq!(Docs::unread(&f.docs.list().unwrap()), 1);
        // An agent's edit makes an old one unread again; the user's doesn't.
        let base = f.docs.read(&old.id.to_string()).unwrap().hash.unwrap();
        f.docs
            .edit(old.id, &base, "changed", &claude(), NOV_9 + 40)
            .unwrap();
        assert_eq!(unread(&mut f.docs), ["New", "Old"]);
        f.docs.seen(NOV_9 + 50, NOV_9 + 50).unwrap();
        let base = f.docs.read(&new.id.to_string()).unwrap().hash.unwrap();
        f.docs
            .edit(new.id, &base, "mine now", &human, NOV_9 + 60)
            .unwrap();
        assert!(unread(&mut f.docs).is_empty());
        // Kept across a restart.
        let mut again = Docs::new(f.base.join("state"));
        assert!(unread(&mut again).is_empty());
    }

    #[test]
    fn a_future_date_cant_pin_a_doc_unread_and_late_docs_stay_unread() {
        let mut f = Fixture::new("unread-edges");
        let root = f.docs.real_root().unwrap();
        std::fs::write(
            root.join("pinned.md"),
            "---\nvalk-id: 5\nby: claude\ncreated: 2100-01-01T00:00:00Z\n---\nhi\n",
        )
        .unwrap();
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs();
        assert_eq!(Docs::unread(&f.docs.list().unwrap()), 1);
        f.docs.seen(now + 1, now + 1).unwrap();
        assert_eq!(
            Docs::unread(&f.docs.list().unwrap()),
            0,
            "cleared by its mtime"
        );
        // The list was stamped before a doc came in: that one stays unread.
        f.docs.seen(NOV_9, now + 1).unwrap();
        assert_eq!(f.docs.config.seen, now + 1, "never goes back");
        let mut g = Fixture::new("unread-late");
        g.docs.seen(NOV_9 + 5, NOV_9 + 99).unwrap();
        g.docs
            .write(&g.new_doc("Late", true), &claude(), NOV_9 + 10)
            .unwrap();
        assert_eq!(Docs::unread(&g.docs.list().unwrap()), 1);
        // A client can't mark the future read.
        g.docs.seen(u64::MAX, NOV_9 + 6).unwrap();
        assert_eq!(g.docs.config.seen, NOV_9 + 6);
    }

    #[test]
    fn a_repos_doc_goes_in_its_folder_with_provenance() {
        let mut f = Fixture::new("write");
        let d = f
            .docs
            .write(&f.new_doc("Docs viewer: plan!", false), &claude(), NOV_9)
            .unwrap();
        assert_eq!(d.id, 1);
        assert_eq!(d.path, "repos/valkyrie/2026-11-09-docs-viewer-plan.md");
        let text = d.text.unwrap();
        assert!(
            text.starts_with("---\nvalk-id: 1\nkind: plan\ntitle: \"Docs viewer: plan!\"\n"),
            "{text}"
        );
        assert!(
            text.contains("by: \"claude\"\nsession: \"#4 docs\"\n"),
            "{text}"
        );
        assert!(text.contains("commit: \"1a2b3c4\"\n"), "{text}");
        assert!(text.contains("item: 132\n"), "{text}");
        assert!(
            text.ends_with("---\n\n# Plan\n\n1. one\n2. two\n"),
            "{text}"
        );

        // Agent-level, and a second of the same title on the same day.
        let g = f
            .docs
            .write(&f.new_doc("Homelab", true), &claude(), NOV_9)
            .unwrap();
        assert_eq!(g.path, "2026-11-09-homelab.md");
        let again = f
            .docs
            .write(&f.new_doc("Homelab", true), &claude(), NOV_9)
            .unwrap();
        assert_eq!(again.path, "2026-11-09-homelab-2.md");
        assert_eq!(again.id, 3);
    }

    #[test]
    fn a_second_repo_with_the_same_name_gets_its_hash() {
        let mut f = Fixture::new("collide");
        f.docs
            .write(&f.new_doc("One", false), &claude(), NOV_9)
            .unwrap();
        let other = f.base.join("work/valkyrie");
        std::fs::create_dir_all(other.join(".git")).unwrap();
        std::fs::write(other.join(".git/HEAD"), "ref: refs/heads/main\n").unwrap();
        let other = valkyrie_proto::canonical(&other).unwrap();
        let mut new = f.new_doc("Two", false);
        new.cwd = other.clone();
        let d = f.docs.write(&new, &claude(), NOV_9).unwrap();
        let key = project::key(&project::root(&other));
        let hash6 = &key[key.len() - 8..key.len() - 2];
        assert_eq!(d.path, format!("repos/valkyrie-{hash6}/2026-11-09-two.md"));
        // Decided once: the first keeps its plain name after a restart.
        let mut docs = Docs::new(f.base.join("state"));
        let d = docs
            .write(&f.new_doc("Three", false), &claude(), NOV_9)
            .unwrap();
        assert_eq!(d.path, "repos/valkyrie/2026-11-09-three.md");
    }

    #[test]
    fn an_edit_must_start_from_what_is_on_disk_and_keeps_the_old_version() {
        let mut f = Fixture::new("edit");
        let d = f
            .docs
            .write(&f.new_doc("Plan", true), &claude(), NOV_9)
            .unwrap();
        let base = d.hash.unwrap();
        let path = f.docs.root().join(&d.path);

        // The user edits it in Obsidian: the agent's stale edit is refused.
        let mine = std::fs::read_to_string(&path)
            .unwrap()
            .replace("2. two", "2. two, mine");
        std::fs::write(&path, &mine).unwrap();
        let err = f
            .docs
            .edit(d.id, &base, "rewritten", &claude(), NOV_9 + 60)
            .unwrap_err();
        assert!(
            err.to_string().contains("changed since you read it"),
            "{err}"
        );
        assert_eq!(std::fs::read_to_string(&path).unwrap(), mine);

        // Read again, fold in, edit: the user's version is kept in history.
        let now = f.docs.read(&d.id.to_string()).unwrap();
        let e = f
            .docs
            .edit(
                d.id,
                now.hash.as_deref().unwrap(),
                "# Plan\n\n2. two, mine and theirs\n",
                &claude(),
                NOV_9 + 120,
            )
            .unwrap();
        let text = e.text.unwrap();
        assert!(
            text.contains("edited:\n  - \"claude, #4 docs, 2026-11-09T21:02:00Z\"\n"),
            "{text}"
        );
        assert!(text.contains("valk-id: 1\n"), "{text}");
        assert!(
            text.ends_with("---\n\n# Plan\n\n2. two, mine and theirs\n"),
            "{text}"
        );
        let history = f.docs.history(d.id);
        assert_eq!(history.len(), 1);
        assert_eq!(std::fs::read_to_string(&history[0].1).unwrap(), mine);
    }

    #[test]
    fn the_users_own_doc_gets_an_id_when_read_and_keeps_its_properties() {
        let mut f = Fixture::new("adopt");
        let root = f.docs.real_root().unwrap();
        std::fs::write(
            root.join("notes.md"),
            "---\ntags: [homelab]\n---\nmy notes\n",
        )
        .unwrap();
        std::fs::write(root.join("bare.md"), "just text\n").unwrap();
        let listed = f.docs.list().unwrap();
        assert!(listed.iter().all(|d| d.id == 0), "{listed:?}");

        let d = f.docs.read("notes.md").unwrap();
        assert_eq!(d.id, 1);
        let text = d.text.unwrap();
        assert!(text.starts_with("---\nvalk-id: 1\nkind: note\ntitle: \"notes\"\nby: \"human\"\ntags: [homelab]\n---\n\nmy notes\n"), "{text}");
        let b = f.docs.read("bare.md").unwrap();
        assert_eq!((b.id, b.title.as_str()), (2, "bare"));
        assert!(b.text.unwrap().ends_with("---\n\njust text\n"));
        // By id from now on.
        assert_eq!(f.docs.read("#1").unwrap().path, "notes.md");
    }

    #[test]
    fn nothing_outside_the_root_is_read() {
        let mut f = Fixture::new("outside");
        std::fs::write(f.base.join("secret.md"), "no").unwrap();
        assert!(f.docs.read("../secret.md").is_err());
        assert!(f.docs.read("/etc/passwd").is_err());
        #[cfg(unix)]
        {
            let root = f.docs.real_root().unwrap();
            std::os::unix::fs::symlink(f.base.join("secret.md"), root.join("link.md")).unwrap();
            assert!(f.docs.read("link.md").is_err());
            // Not listed either: symlinks aren't followed.
            assert!(f.docs.list().unwrap().is_empty());
        }
    }

    #[test]
    fn a_root_must_be_absolute_and_not_home() {
        let mut f = Fixture::new("root");
        assert!(f.docs.set_root(Some(Path::new("vault"))).is_err());
        assert!(f.docs.set_root(Some(&valkyrie_proto::home_dir())).is_err());
        let vault = f.base.join("vault/valk");
        let set = f.docs.set_root(Some(&vault)).unwrap();
        assert_eq!(set, valkyrie_proto::canonical(&vault).unwrap());
        assert!(Docs::new(f.base.join("state")).root_is_set());
        let d = f
            .docs
            .write(&f.new_doc("In the vault", true), &claude(), NOV_9)
            .unwrap();
        assert!(vault.join(&d.path).is_file());
    }

    #[test]
    fn history_keeps_fifty() {
        let f = Fixture::new("history");
        for i in 0..55 {
            f.docs.keep(7, &format!("v{i}"), NOV_9 + i).unwrap();
        }
        let h = f.docs.history(7);
        assert_eq!(h.len(), 50);
        assert_eq!(std::fs::read_to_string(&h[0].1).unwrap(), "v54");
    }

    #[test]
    fn titles_and_bodies_are_cleaned() {
        assert!(clean_title("").is_err());
        assert_eq!(clean_title("a\u{202e}b\nc").unwrap(), "a b c");
        assert!(clean_body(&"x".repeat(MAX_BODY + 1)).is_err());
        assert_eq!(clean_body("a\u{1b}[31mb\n").unwrap(), "a[31mb\n");
        assert_eq!(slug("Docs: the Plan (v2)!"), "docs-the-plan-v2");
        assert_eq!(slug("!!!"), "doc");
    }
}
