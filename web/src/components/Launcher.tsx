import { useEffect, useLayoutEffect, useRef, useState } from "react";
import { getJson, type Listing, type Place, type Places } from "../lib/api";
import { spawn, toast, useApp } from "../store";
import { Back, Chevron } from "./icons";
import { TutorButton } from "./Tutor";
import { MicButton } from "./Mic";
import { QuotaMeter } from "./Quota";
import { useRepos } from "./Repos";
import { isKnown, isScanned } from "../lib/repos";
import type { RepoEdit, RepoList } from "../proto";
import { HANDOFF_KEY, type Handoff } from "./Composer";

type Agent = "claude" | "codex" | "shell";

const AGENTS: { id: Agent; label: string }[] = [
  { id: "claude", label: "Claude Code" },
  { id: "codex", label: "Codex" },
  { id: "shell", label: "Shell" },
];

const LAST_KEY = "valk.launch";

interface Last {
  agent: Agent;
  folder: string;
}

function lastUsed(): Partial<Last> {
  try {
    return JSON.parse(localStorage.getItem(LAST_KEY) ?? "{}") as Partial<Last>;
  } catch {
    return {};
  }
}

function remember(last: Last): void {
  try {
    localStorage.setItem(LAST_KEY, JSON.stringify(last));
  } catch {
    /* remembered for this visit only */
  }
}

function takeHandoff(): Handoff | null {
  try {
    const v = sessionStorage.getItem(HANDOFF_KEY);
    sessionStorage.removeItem(HANDOFF_KEY);
    return v ? (JSON.parse(v) as Handoff) : null;
  } catch {
    return null;
  }
}

const back = () => (history.length > 1 ? history.back() : (location.hash = "#/"));

/**
 * A new session from the phone (DESIGN §8.8): an agent, a folder, and optionally
 * what to ask it first, which goes on the agent's command line so it starts
 * working at once.
 */
export function Launcher() {
  // Several lines from the home composer (#275), once.
  const [handoff] = useState(takeHandoff);
  const token = useApp((s) => s.token);
  const status = useApp((s) => s.status);
  const [agent, setAgent] = useState<Agent>(() => (handoff?.agent as Agent | undefined) ?? lastUsed().agent ?? "claude");
  const [folder, setFolder] = useState(() => handoff?.folder ?? lastUsed().folder ?? "");
  const [name, setName] = useState("");
  // Not remembered: the next folder may not be a repository.
  const [worktree, setWorktree] = useState(!!handoff);
  const [message, setMessage] = useState(handoff?.message ?? "");
  const [places, setPlaces] = useState<Places | null>(null);
  const [browsing, setBrowsing] = useState<Listing | null>(null);
  const [busy, setBusy] = useState(false);
  const { list: repos, edit } = useRepos();
  const area = useRef<HTMLTextAreaElement>(null);

  useEffect(() => {
    if (!token) return;
    getJson<Places>(token, "/api/places").then(
      (p) => {
        setPlaces(p);
        setFolder((f) => f || p.recent[0]?.path || p.home);
      },
      (e: Error) => toast(`Can't list folders: ${e.message}`),
    );
  }, [token]);

  useLayoutEffect(() => {
    const el = area.current;
    if (!el) return;
    el.style.height = "auto";
    el.style.height = `${Math.min(el.scrollHeight, 200)}px`;
  }, [message, agent]);

  const browse = async (path: string) => {
    if (!token) return;
    try {
      setBrowsing(await getJson<Listing>(token, `/api/dirs?path=${encodeURIComponent(path)}`));
    } catch (e) {
      toast(`Can't open ${path}: ${(e as Error).message}`);
    }
  };

  const start = async () => {
    if (busy || !folder.trim() || !places) return;
    const text = message.trim();
    const command =
      agent === "shell" ? [places.shell] : text ? [agent, text] : [agent];
    setBusy(true);
    try {
      const path = expand(folder, places.home);
      const id = await spawn(command, path, name.trim() || null, worktree ? "" : null);
      remember({ agent, folder: path });
      navigator.vibrate?.(10);
      location.replace(`#/s/${id}`);
    } catch (e) {
      toast(`Couldn't start: ${(e as Error).message}`);
    } finally {
      setBusy(false);
    }
  };

  const pick = (p: Place) => {
    setFolder(p.path);
    setBrowsing(null);
  };
  const label = places ? shortLabel(folder, places.home) : folder;
  const agentLabel = AGENTS.find((a) => a.id === agent)!.label;

  return (
    <div
      className="page launcher-page"
      onKeyDown={(e) => {
        if (e.key === "Enter" && (e.ctrlKey || e.metaKey)) {
          e.preventDefault();
          void start();
        }
      }}
    >
      <header className="topbar">
        <button className="icon-btn" aria-label="Back" onClick={back}>
          <Back />
        </button>
        <div className="title">
          <h1 className="title-name">New session</h1>
        </div>
      </header>

      <main className="launcher">
        {agent === "claude" && <QuotaMeter />}
        <div className="modes agent-pick" role="radiogroup" aria-label="Agent">
          {AGENTS.map((a) => (
            <button key={a.id} role="radio" aria-checked={agent === a.id} className={agent === a.id ? "on" : ""} onClick={() => setAgent(a.id)}>
              {a.label}
            </button>
          ))}
        </div>

        <label className="field">
          <span className="field-label">Folder</span>
          <input
            value={places ? shortLabel(folder, places.home) : folder}
            onChange={(e) => setFolder(e.target.value)}
            placeholder="~/repos/project"
            autoCapitalize="off"
            autoCorrect="off"
            spellCheck={false}
          />
        </label>

        {browsing ? (
          <div className="folders">
            <div className="folders-head">
              {/* Cut from the left, so the folder's own name shows; the marks keep the slashes in place. */}
              <span className="folders-where">{`\u200e${browsing.label}\u200e`}</span>
              <button className="ghost" onClick={() => setBrowsing(null)}>
                Done
              </button>
            </div>
            {browsing.parent && (
              <button className="folder" onClick={() => void browse(browsing.parent!)}>
                <span className="folder-name">..</span>
              </button>
            )}
            <button className="folder use" onClick={() => pick(browsing)}>
              <span className="folder-name">Use {browsing.label}</span>
              {browsing.git && <span className="git">git</span>}
            </button>
            {browsing.git && !isKnown(repos, browsing.path) && (
              <button className="folder use" onClick={() => void edit({ do: "add", path: browsing.path })}>
                <span className="folder-name">Add {browsing.label} to repos</span>
              </button>
            )}
            {!browsing.git && !isScanned(repos, browsing.path) && (
              <button className="folder use" onClick={() => void edit({ do: "scan", path: browsing.path })}>
                <span className="folder-name">Find repos in {browsing.label}</span>
              </button>
            )}
            {browsing.dirs.map((d) => (
              <button key={d.path} className="folder" onClick={() => void browse(d.path)}>
                <span className="folder-name">{d.label.split("/").pop()}</span>
                {d.git && <span className="git">git</span>}
                <Chevron />
              </button>
            ))}
            {browsing.dirs.length === 0 && <div className="empty">No folders here.</div>}
          </div>
        ) : (
          <div className="folders">
            <FolderGroup title="Open in sessions" places={places?.recent ?? []} current={folder} onPick={pick} />
            <FolderGroup
              title="Repos"
              places={(repos?.repos ?? []).map((r) => ({ ...r, git: true }))}
              current={folder}
              onPick={pick}
              onHide={(p) => void edit({ do: "hide", path: p.path })}
            />
            <button className="folder browse" onClick={() => void browse(folder || places?.home || "~")}>
              <span className="folder-name">Browse from {label || "~"}…</span>
              <Chevron />
            </button>
            <RepoSources list={repos} onEdit={(e) => void edit(e)} />
          </div>
        )}

        <label className="field">
          <span className="field-label">Name</span>
          <input
            value={name}
            onChange={(e) => setName(e.target.value)}
            placeholder={worktree ? "Also its branch" : "Its folder"}
          />
        </label>

        <label className="check-row">
          <input type="checkbox" checked={worktree} onChange={(e) => setWorktree(e.target.checked)} />
          <span className="check-what">
            <span className="check-name">Own worktree</span>
            <span className="check-about">A new branch and checkout beside the repo, so it can't touch other agents' files</span>
          </span>
        </label>

        {agent !== "shell" && (
          <label className="field">
            <span className="field-label">First message</span>
            <div className="field-row">
              <textarea
                ref={area}
                rows={2}
                value={message}
                onChange={(e) => setMessage(e.target.value)}
                placeholder={`What should ${agentLabel} do? (optional)`}
              />
              <MicButton value={message} onChange={setMessage} />
            </div>
          </label>
        )}
        <TutorButton className="ghost launcher-tutor" label="New to Valkyrie? Start the tutor" />
      </main>

      <footer className="dock">
        <button className="primary start" disabled={busy || !folder.trim() || !places || status !== "open"} onClick={() => void start()}>
          {busy ? "Starting…" : `Start ${agentLabel} in ${label || "…"}`}
        </button>
      </footer>
    </div>
  );
}

function FolderGroup({
  title,
  places,
  current,
  onPick,
  onHide,
}: {
  title: string;
  places: Place[];
  current: string;
  onPick: (p: Place) => void;
  /** Stops offering one, with a button on its row. */
  onHide?: (p: Place) => void;
}) {
  const [all, setAll] = useState(false);
  if (places.length === 0) return null;
  const shown = all ? places : places.slice(0, 5);
  return (
    <>
      <div className="folders-title">{title}</div>
      {shown.map((p) =>
        onHide ? (
          <div key={p.path} className={`folder-row ${p.path === current ? "picked" : ""}`}>
            <button className="folder" onClick={() => onPick(p)}>
              <span className="folder-name">{p.label}</span>
              {p.git && <span className="git">git</span>}
            </button>
            <button className="folder-hide" aria-label={`Hide ${p.label}`} title="Don't offer this repo" onClick={() => onHide(p)}>
              ×
            </button>
          </div>
        ) : (
          <button key={p.path} className={`folder ${p.path === current ? "picked" : ""}`} onClick={() => onPick(p)}>
            <span className="folder-name">{p.label}</span>
            {p.git && <span className="git">git</span>}
          </button>
        ),
      )}
      {places.length > shown.length && (
        <button className="folder more" onClick={() => setAll(true)}>
          <span className="folder-name">{places.length - shown.length} more</span>
        </button>
      )}
    </>
  );
}

/** Where the repos come from: the folders scanned, each one you can stop, and
 * the repos hidden, each one you can bring back. Folded until asked for. */
function RepoSources({ list, onEdit }: { list: RepoList | null; onEdit: (e: RepoEdit) => void }) {
  const [open, setOpen] = useState(false);
  if (!list) return null;
  const n = list.folders.length;
  const hidden = list.hidden.length;
  const summary = `Finding repos in ${n} folder${n === 1 ? "" : "s"}${hidden ? `, ${hidden} hidden` : ""}`;
  return (
    <>
      <button className="folder more" aria-expanded={open} onClick={() => setOpen(!open)}>
        <span className="folder-name">{summary}</span>
      </button>
      {open &&
        list.folders.map((f) => (
          <div key={f.path} className="folder-row">
            <span className="folder">
              <span className="folder-name">{f.label}</span>
            </span>
            <button className="folder-hide" aria-label={`Stop looking in ${f.label}`} title="Stop looking here" onClick={() => onEdit({ do: "unscan", path: f.path })}>
              ×
            </button>
          </div>
        ))}
      {open && n === 0 && <div className="empty">No folders. Browse to one and pick "Find repos in" it.</div>}
      {open && hidden > 0 && <div className="folders-title">Hidden</div>}
      {open &&
        list.hidden.map((h) => (
          <div key={h.path} className="folder-row">
            <span className="folder">
              <span className="folder-name">{h.label}</span>
            </span>
            <button className="ghost folder-show" onClick={() => onEdit({ do: "unhide", path: h.path })}>
              Show
            </button>
          </div>
        ))}
    </>
  );
}

/** `/home/u/repos/x` reads as `~/repos/x`. */
export function shortLabel(path: string, home: string): string {
  if (path === home) return "~";
  return path.startsWith(home + "/") ? `~${path.slice(home.length)}` : path;
}

/** `~/repos/x` → `/home/u/repos/x`; the daemon wants an absolute path. */
export function expand(path: string, home: string): string {
  const p = path.trim();
  if (p === "~") return home;
  return p.startsWith("~/") ? home + p.slice(1) : p;
}
