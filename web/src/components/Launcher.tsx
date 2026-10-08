import { useEffect, useLayoutEffect, useRef, useState } from "react";
import { getJson, type Listing, type Place, type Places } from "../lib/api";
import { spawn, toast, useApp } from "../store";
import { Back, Chevron } from "./icons";
import { MicButton } from "./Mic";

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

const back = () => (history.length > 1 ? history.back() : (location.hash = "#/"));

/**
 * A new session from the phone (DESIGN §8.8): an agent, a folder, and optionally
 * what to ask it first, which goes on the agent's command line so it starts
 * working at once.
 */
export function Launcher() {
  const token = useApp((s) => s.token);
  const status = useApp((s) => s.status);
  const [agent, setAgent] = useState<Agent>(() => lastUsed().agent ?? "claude");
  const [folder, setFolder] = useState(() => lastUsed().folder ?? "");
  const [name, setName] = useState("");
  const [message, setMessage] = useState("");
  const [places, setPlaces] = useState<Places | null>(null);
  const [browsing, setBrowsing] = useState<Listing | null>(null);
  const [busy, setBusy] = useState(false);
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
      const id = await spawn(command, path, name.trim() || null);
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
          <div className="title-name">New session</div>
        </div>
      </header>

      <main className="launcher">
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
            <FolderGroup title="Repos" places={places?.repos ?? []} current={folder} onPick={pick} />
            <button className="folder browse" onClick={() => void browse(folder || places?.home || "~")}>
              <span className="folder-name">Browse from {label || "~"}…</span>
              <Chevron />
            </button>
          </div>
        )}

        <label className="field">
          <span className="field-label">Name</span>
          <input value={name} onChange={(e) => setName(e.target.value)} placeholder="Its folder" />
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
}: {
  title: string;
  places: Place[];
  current: string;
  onPick: (p: Place) => void;
}) {
  const [all, setAll] = useState(false);
  if (places.length === 0) return null;
  const shown = all ? places : places.slice(0, 5);
  return (
    <>
      <div className="folders-title">{title}</div>
      {shown.map((p) => (
        <button key={p.path} className={`folder ${p.path === current ? "picked" : ""}`} onClick={() => onPick(p)}>
          <span className="folder-name">{p.label}</span>
          {p.git && <span className="git">git</span>}
        </button>
      ))}
      {places.length > shown.length && (
        <button className="folder more" onClick={() => setAll(true)}>
          <span className="folder-name">{places.length - shown.length} more</span>
        </button>
      )}
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
