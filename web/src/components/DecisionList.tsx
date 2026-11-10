import { useCallback, useEffect, useState } from "react";
import type { Decision } from "../proto";
import { byProject, inFull, pinnable } from "../lib/decisions";
import { scopeName } from "../lib/scope";
import { allDecisions, review, useApp } from "../store";
import { Back } from "./icons";
import { Markdown } from "./Markdown";

const home = () => {
  location.hash = "#/";
};

/**
 * Every active decision, to pin the ones agents should always get in full
 * (ADR-0014 §1). The rest reach an agent as a title it can read on demand.
 */
export function DecisionList() {
  const status = useApp((s) => s.status);
  const [list, setList] = useState<Decision[] | null>(null);
  const [failed, setFailed] = useState(false);
  const [only, setOnly] = useState<string | null>(null);

  const load = useCallback(async () => {
    const got = await allDecisions();
    setFailed(got === null);
    if (got) setList(got);
  }, []);

  useEffect(() => {
    if (status === "open") void load();
  }, [status, load]);

  // q or Esc goes back, when you aren't typing.
  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if ((e.target as HTMLElement).closest("textarea, input") || e.ctrlKey || e.metaKey || e.altKey) return;
      if (e.key === "q" || e.key === "Escape") {
        e.preventDefault();
        home();
      }
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, []);

  const groups = list ? byProject(list) : [];
  const shown = groups.filter(([p]) => only === null || p === only);

  return (
    <div className="page">
      <header className="topbar">
        <button className="icon-btn" aria-label="Back" onClick={home}>
          <Back />
        </button>
        <div className="title">
          <div className="title-name">Decisions</div>
        </div>
      </header>

      <main className="inbox">
        <p className="decisions-about">
          Every agent gets constraints and the decisions you pin in full when it starts. The rest it gets as a title and
          reads when it needs to. Each pin takes up room in every session, so pin what holds everywhere.
        </p>
        {groups.length > 1 && (
          <div className="kind-pick" role="radiogroup" aria-label="Project">
            {[null, ...groups.map(([p]) => p)].map((p) => (
              <button
                key={p ?? "all"}
                role="radio"
                aria-checked={only === p}
                className={`kind-chip ${only === p ? "on" : ""}`}
                onClick={() => setOnly(p)}
              >
                {p === null ? "all" : scopeName(p)}
              </button>
            ))}
          </div>
        )}
        {list === null && <div className="empty">{failed ? "Couldn't load the decisions." : "Loading…"}</div>}
        {list !== null && groups.length === 0 && <div className="empty">No active decisions yet.</div>}
        {shown.map(([project, ds]) => (
          <section key={project}>
            <h2 className="section-title">
              {scopeName(project)} <span className="count">{ds.length}</span>
            </h2>
            <div className="cards">
              {ds.map((d) => (
                <DecisionRow key={`${d.project}#${d.id}`} d={d} onChange={load} />
              ))}
            </div>
          </section>
        ))}
      </main>
    </div>
  );
}

function DecisionRow({ d, onChange }: { d: Decision; onChange: () => Promise<void> }) {
  const [open, setOpen] = useState(false);
  const [busy, setBusy] = useState(false);
  // Read again either way: a refusal ("changed since you read it") is fixed by
  // what's there now.
  const toggle = async () => {
    setBusy(true);
    try {
      await review(d, { a: d.pinned ? "unpin" : "pin" });
      await onChange();
    } finally {
      setBusy(false);
    }
  };
  const how = d.kind === "constraint" ? "always in full" : d.pinned ? "pinned" : null;
  return (
    <div className="card-slot">
      <article className={`card decision-card ${inFull(d) ? "in-full" : ""}`}>
        <div className="card-head">
          <span className={`kind-chip kind-${d.kind}`}>{d.kind}</span>
          <span className="card-name">#{d.id}</span>
          {how && <span className="card-prog">{how}</span>}
        </div>
        <button className="decision-title as-text" aria-expanded={open} onClick={() => setOpen(!open)}>
          {d.title}
        </button>
        {open && d.body && <Markdown text={d.body} className={`decision-body ${d.kind}`} />}
        {d.scope?.length ? <div className="decision-scope">applies to {d.scope.join(", ")}</div> : null}
        {pinnable(d) && (
          <div className="card-actions">
            <button className={d.pinned ? "ghost" : "primary"} disabled={busy} onClick={() => void toggle()}>
              {d.pinned ? "Unpin" : "Pin"}
            </button>
          </div>
        )}
      </article>
    </div>
  );
}
