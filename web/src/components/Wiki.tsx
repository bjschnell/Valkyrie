import { useEffect, useState } from "react";
import type { WikiListing, WikiPage } from "../proto";
import { toast, useApp, wikiExplore, wikiListing, wikiPage } from "../store";
import { useCached } from "../cache";
import { exploreLabel, staleCount, wikiRepos, wikiSummary } from "../lib/wiki";
import { Loading } from "./bits";
import { EmptyState, ErrorState } from "./states";
import { Back } from "./icons";
import { Markdown } from "./Markdown";
import { usePageKeys } from "../lib/pageKeys";

const REPO_KEY = "valk.wiki.repo";

function readRepo(): string | null {
  try {
    return localStorage.getItem(REPO_KEY);
  } catch {
    return null;
  }
}

function keepRepo(repo: string) {
  try {
    localStorage.setItem(REPO_KEY, repo);
  } catch {
    // A private window: the pick lasts the visit.
  }
}

const home = () => {
  location.hash = "#/";
};

/**
 * The repository wiki (backlog #210): pages agents wrote about how a
 * repository is built, stale ones marked. From here you start the agent that
 * maps the repository, or one that rewrites only the stale pages; either asks a
 * second tap first, since it starts an agent, then opens its session.
 */
export function WikiView() {
  const items = useApp((s) => s.items);
  const sessions = useApp((s) => s.sessions);
  const choices = wikiRepos(items, sessions);
  const [picked, setPicked] = useState<string | null>(readRepo);
  const repo = picked !== null && choices.some((c) => c.repo === picked) ? picked : (choices[0]?.repo ?? null);
  const [reading, setReading] = useState<WikiPage | null>(null);

  const back = () => (reading ? setReading(null) : home());
  usePageKeys("wiki", { q: back, Escape: back });

  const pick = (r: string) => {
    keepRepo(r);
    setPicked(r);
    setReading(null);
  };

  return (
    <div className="page">
      <header className="topbar">
        <button className="icon-btn" aria-label={reading ? "Back to the wiki" : "Back"} onClick={back}>
          <Back />
        </button>
        <div className="title">
          <h1 className="title-name">{reading ? reading.title : "Wiki"}</h1>
          {reading?.summary && <div className="title-sub">{reading.summary}</div>}
        </div>
      </header>
      <main className={`inbox ${reading ? "doc-page" : ""}`}>
        {repo === null && (
          <EmptyState
            title="No repository to show"
            hint="The wiki is a repository's. Start a session in one, or write a backlog item for one, and it shows up here."
            action={{ label: "Start a session", run: () => (location.hash = "#/new") }}
          />
        )}
        {repo !== null && reading && <Reader page={reading} />}
        {repo !== null && !reading && (
          <Listing repo={repo} choices={choices} onPick={pick} onRead={setReading} />
        )}
      </main>
    </div>
  );
}

function Listing({
  repo,
  choices,
  onPick,
  onRead,
}: {
  repo: string;
  choices: { repo: string; label: string }[];
  onPick: (repo: string) => void;
  onRead: (page: WikiPage) => void;
}) {
  const { data: listing, failed, reload } = useCached(`wiki:${repo}`, () => wikiListing(repo));
  const [opening, setOpening] = useState<string | null>(null);

  const read = async (slug: string) => {
    if (opening !== null) return;
    setOpening(slug);
    try {
      onRead(await wikiPage(repo, slug));
    } catch (e) {
      toast(`Couldn't open it: ${(e as Error).message}`);
    } finally {
      setOpening(null);
    }
  };

  return (
    <>
      {choices.length > 1 && (
        <div className="backlog-filter">
          <select aria-label="Repository" value={repo} onChange={(e) => onPick(e.target.value)}>
            {choices.map((c) => (
              <option key={c.repo} value={c.repo} title={c.repo}>
                {c.label}
              </option>
            ))}
          </select>
        </div>
      )}
      {listing === null && (failed ? <ErrorState what="the wiki" why={failed} onRetry={reload} /> : <Loading rows={5} />)}
      {listing !== null && <Actions repo={repo} listing={listing} />}
      {listing !== null && listing.pages.length === 0 && (
        <EmptyState
          title="No wiki yet"
          hint="Map this repo and an agent explores it and writes pages on how it's built: an overview, then one per part. Every agent started here then gets their index, and reads the pages its task needs instead of exploring from scratch."
        />
      )}
      {listing !== null && listing.pages.length > 0 && (
        <div className="list">
          {listing.pages.map((p) => (
            <button key={p.slug} className="row doc-row" disabled={opening !== null} onClick={() => void read(p.slug)}>
              <span className={`kind-chip ${p.stale ? "wiki-stale" : ""}`}>{opening === p.slug ? "opening…" : p.stale ? "stale" : p.slug}</span>
              <span className="row-main">
                <span className="row-name">{p.title}</span>
                <span className="row-sub">{p.stale ? `Stale: ${p.stale}` : p.summary || p.scope.join(", ")}</span>
              </span>
            </button>
          ))}
        </div>
      )}
    </>
  );
}

/** The two ways to start the agent. Each asks a second tap, then opens it. */
function Actions({ repo, listing }: { repo: string; listing: WikiListing }) {
  const [armed, setArmed] = useState<boolean | null>(null);
  const [busy, setBusy] = useState(false);
  // An armed button that isn't tapped again goes back to what it was.
  useEffect(() => {
    if (armed === null) return;
    const t = setTimeout(() => setArmed(null), 5000);
    return () => clearTimeout(t);
  }, [armed]);

  const start = async (stale: boolean) => {
    if (busy) return;
    if (armed !== stale) {
      setArmed(stale);
      return;
    }
    setBusy(true);
    try {
      const id = await wikiExplore(repo, stale);
      location.hash = `#/s/${id}`;
    } catch (e) {
      toast(`Couldn't start the wiki agent: ${(e as Error).message}`);
      setBusy(false);
      setArmed(null);
    }
  };

  const stale = staleCount(listing);
  const button = (forStale: boolean) => {
    const label = exploreLabel(listing, forStale);
    const sure = armed === forStale;
    return (
      <button
        key={String(forStale)}
        className={sure ? "primary" : "ghost"}
        disabled={busy}
        title={label.confirm}
        onClick={() => void start(forStale)}
      >
        {busy && sure ? "Starting…" : sure ? `${label.confirm} Tap again` : label.button}
      </button>
    );
  };
  return (
    <div className="wiki-actions">
      <span className="digest-about">{wikiSummary(listing)}</span>
      <span className="wiki-buttons">
        {stale > 0 && button(true)}
        {button(false)}
      </span>
    </div>
  );
}

function Reader({ page }: { page: WikiPage }) {
  const covers = page.scope.length ? page.scope.join(", ") : "no paths";
  return (
    <>
      <div className="doc-meta">
        <span className={`kind-chip ${page.stale ? "wiki-stale" : ""}`}>{page.stale ? "stale" : page.slug}</span>
        {page.commit ? <code>{page.commit}</code> : null}
        <span className="doc-path">covers {covers}</span>
      </div>
      {page.stale && <p className="wiki-stale-why">Stale: {page.stale}. Check it against the code before relying on it.</p>}
      <Markdown text={page.body} className="doc-body" />
    </>
  );
}
