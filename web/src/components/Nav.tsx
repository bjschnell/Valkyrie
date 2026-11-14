// #104 trial ("Navigation"): one place to move between the app's pages. A phone
// gets a tab bar at the thumb with the rest under More; a desktop gets a rail.

import { useEffect, useState, type ReactNode } from "react";
import { prefetch } from "../cache";
import { allDocs, pastSessions, supervisorLog, useApp } from "../store";
import { fetchMemory } from "./DecisionList";
import { Clock, Dots, Eye, Gear, List, Page, Pin, Plus, Rewind, Search, Tray } from "./icons";

export type NavKey = "inbox" | "backlog" | "docs" | "memory" | "search" | "digest" | "past" | "supervisor" | "settings";

interface Dest {
  key: NavKey;
  hash: string;
  label: string;
  icon: () => ReactNode;
  /** Warms the page's data on hover or press, so it opens with it. */
  warm?: () => void;
}

const DESTS: Record<NavKey, Dest> = {
  inbox: { key: "inbox", hash: "#/", label: "Inbox", icon: Tray },
  backlog: { key: "backlog", hash: "#/backlog", label: "Backlog", icon: List },
  docs: { key: "docs", hash: "#/docs", label: "Docs", icon: Page, warm: () => prefetch("docs", allDocs) },
  memory: {
    key: "memory",
    hash: "#/decisions",
    label: "Memory",
    icon: Pin,
    warm: () => prefetch("memory", fetchMemory),
  },
  search: { key: "search", hash: "#/search", label: "Search", icon: Search },
  digest: { key: "digest", hash: "#/digest", label: "Digest", icon: Clock },
  past: { key: "past", hash: "#/history", label: "History", icon: Rewind, warm: () => prefetch("past", pastSessions) },
  supervisor: {
    key: "supervisor",
    hash: "#/supervisor",
    label: "Supervisor",
    icon: Eye,
    warm: () => prefetch("supervisor-log", supervisorLog),
  },
  settings: { key: "settings", hash: "#/settings", label: "Settings", icon: Gear },
};

/** On a phone's bar; the rest live under More. */
const TABS: NavKey[] = ["inbox", "backlog", "docs", "memory"];
const MORE: NavKey[] = ["search", "digest", "past", "supervisor", "settings"];
/** Down a desktop's rail, then Settings at its foot. */
const RAIL: NavKey[] = ["inbox", "backlog", "docs", "memory", "search", "digest", "past", "supervisor"];

function useBadges(): Partial<Record<NavKey, number>> {
  const needs = useApp((s) => s.queue.length);
  const open = useApp((s) => s.items.filter((i) => i.status === "open").length);
  const docs = useApp((s) => s.unreadDocs);
  return { inbox: needs, backlog: open, docs };
}

function go(d: Dest) {
  if (location.hash !== d.hash && !(d.hash === "#/" && (location.hash === "" || location.hash === "#"))) {
    location.hash = d.hash;
  }
}

function NavButton({ d, at, badge, className }: { d: Dest; at: NavKey | null; badge?: number; className: string }) {
  const Icon = d.icon;
  const here = at === d.key;
  return (
    <button
      className={className}
      aria-current={here ? "page" : undefined}
      aria-label={badge ? `${d.label}, ${badge}` : d.label}
      title={d.label}
      onPointerEnter={d.warm}
      onPointerDown={d.warm}
      onClick={() => go(d)}
    >
      <span className="nav-icon">
        <Icon />
        {!!badge && <span className={`nav-badge${d.key === "inbox" ? " needs" : ""}`}>{badge > 99 ? "99+" : badge}</span>}
      </span>
      <span className="nav-label">{d.label}</span>
    </button>
  );
}

/** The desktop's rail: every page, New at the top, Settings at the foot. */
export function Rail({ at }: { at: NavKey | null }) {
  const badges = useBadges();
  return (
    <nav className="rail" aria-label="Pages">
      <button className="rail-new" aria-label="New session" title="New session" onClick={() => (location.hash = "#/new")}>
        <Plus />
      </button>
      {RAIL.map((k) => (
        <NavButton key={k} d={DESTS[k]} at={at} badge={badges[k]} className="rail-item" />
      ))}
      <div className="rail-gap" />
      <NavButton d={DESTS.settings} at={at} className="rail-item" />
    </nav>
  );
}

/** The phone's tab bar, with More opening a sheet of the other pages. */
export function TabBar({ at }: { at: NavKey | null }) {
  const badges = useBadges();
  const [more, setMore] = useState(false);
  const inMore = at !== null && MORE.includes(at);
  useEffect(() => setMore(false), [at]);
  useEffect(() => {
    if (!more) return;
    const onKey = (e: KeyboardEvent) => e.key === "Escape" && setMore(false);
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [more]);
  return (
    <>
      {more && (
        <div className="sheet-backdrop" onClick={() => setMore(false)}>
          <div className="sheet" role="dialog" aria-label="More pages" onClick={(e) => e.stopPropagation()}>
            <div className="sheet-grip" />
            <div className="list">
              {MORE.map((k) => {
                const d = DESTS[k];
                const Icon = d.icon;
                return (
                  <button
                    key={k}
                    className="row sheet-row"
                    aria-current={at === k ? "page" : undefined}
                    onPointerDown={d.warm}
                    onClick={() => {
                      setMore(false);
                      go(d);
                    }}
                  >
                    <span className="sheet-icon">
                      <Icon />
                    </span>
                    <span className="row-main">
                      <span className="row-name">{d.label}</span>
                    </span>
                  </button>
                );
              })}
            </div>
          </div>
        </div>
      )}
      <nav className="tabbar" aria-label="Pages">
        {TABS.map((k) => (
          <NavButton key={k} d={DESTS[k]} at={at} badge={badges[k]} className="tab" />
        ))}
        <button
          className="tab"
          aria-current={inMore ? "page" : undefined}
          aria-expanded={more}
          aria-label="More pages"
          onClick={() => setMore((m) => !m)}
        >
          <span className="nav-icon">
            <Dots />
          </span>
          <span className="nav-label">More</span>
        </button>
      </nav>
    </>
  );
}
