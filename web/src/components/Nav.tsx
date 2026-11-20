// One place to move between the app's four sections (#104, #249): Inbox,
// Backlog, Memory and History. A phone gets them as a tab bar at the thumb, a
// desktop as a rail with New at its head and Settings at its foot. The pages
// inside a section are tabs at the top of it (SectionTabs).

import { type ReactNode } from "react";
import { prefetch } from "../cache";
import { HASH } from "../lib/routes";
import { allDocs, pastSessions, supervisorLog, useApp } from "../store";
import { fetchMemory } from "./DecisionList";
import { HostButton } from "./Hosts";
import { useAttention } from "./Attention";
import { Gear, List, Pin, Plus, Rewind, Tray } from "./icons";

export type NavKey = "inbox" | "backlog" | "memory" | "history" | "settings";

interface Dest {
  key: NavKey;
  hash: string;
  label: string;
  icon: () => ReactNode;
  /** Warms the page's data on hover or press, so it opens with it. */
  warm?: () => void;
}

const DESTS: Record<NavKey, Dest> = {
  inbox: { key: "inbox", hash: HASH.inbox, label: "Inbox", icon: Tray },
  backlog: { key: "backlog", hash: HASH.backlog, label: "Backlog", icon: List },
  memory: { key: "memory", hash: HASH.memory, label: "Memory", icon: Pin, warm: () => prefetch("memory", fetchMemory) },
  history: { key: "history", hash: HASH.history, label: "History", icon: Rewind, warm: () => prefetch("past", pastSessions) },
  settings: { key: "settings", hash: HASH.settings, label: "Settings", icon: Gear },
};

/** The four sections, down the rail and along the phone's tab bar. */
const SECTIONS: NavKey[] = ["inbox", "backlog", "memory", "history"];

/** A section's tabs: what each one is called and where it lives. */
export interface Tab {
  label: string;
  hash: string;
  warm?: () => void;
}

export const MEMORY_TABS: Tab[] = [
  { label: "Decisions", hash: HASH.memory, warm: DESTS.memory.warm },
  { label: "Docs", hash: HASH.docs, warm: () => prefetch("docs", allDocs) },
  { label: "Wiki", hash: HASH.wiki },
];

export const HISTORY_TABS: Tab[] = [
  { label: "Sessions", hash: HASH.history, warm: DESTS.history.warm },
  { label: "Summary", hash: HASH.summary },
  { label: "Search", hash: HASH.search },
  { label: "Supervisor", hash: HASH.supervisor, warm: () => prefetch("supervisor-log", supervisorLog) },
];

/** Every page, for the command palette. */
export const PAGES = [
  { label: "Inbox", hash: HASH.inbox },
  { label: "Backlog", hash: HASH.backlog },
  ...MEMORY_TABS.map((t) => ({ label: t.label === "Decisions" ? "Memory" : `Memory: ${t.label}`, hash: t.hash })),
  ...HISTORY_TABS.map((t) => ({ label: t.label === "Sessions" ? "History" : `History: ${t.label}`, hash: t.hash })),
  { label: "Settings", hash: HASH.settings },
];

/** The rail's one badge: everything waiting on you, on the Inbox (#251). Nothing
 * else gets a count of its own. */
function useBadges(): Partial<Record<NavKey, number>> {
  return { inbox: useAttention().length };
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

/** The desktop's rail: the four sections, New at the top, Settings at the foot. */
export function Rail({ at }: { at: NavKey | null }) {
  const badges = useBadges();
  return (
    <nav className="rail" aria-label="Sections">
      <button className="rail-new" aria-label="New session" title="New session" onClick={() => (location.hash = "#/new")}>
        <Plus />
      </button>
      {SECTIONS.map((k) => (
        <NavButton key={k} d={DESTS[k]} at={at} badge={badges[k]} className="rail-item" />
      ))}
      <div className="rail-gap" />
      <HostButton />
      <NavButton d={DESTS.settings} at={at} className="rail-item" />
    </nav>
  );
}

/** The phone's tab bar: exactly the four sections. Settings is a gear on the Inbox. */
export function TabBar({ at }: { at: NavKey | null }) {
  const badges = useBadges();
  return (
    <nav className="tabbar" aria-label="Sections">
      {SECTIONS.map((k) => (
        <NavButton key={k} d={DESTS[k]} at={at} badge={badges[k]} className="tab" />
      ))}
    </nav>
  );
}

/**
 * The pages inside a section, as tabs under its top bar. `counts` puts a number
 * on a tab (unread docs), never a badge on the nav.
 */
export function SectionTabs({ tabs, label, counts }: { tabs: Tab[]; label: string; counts?: Record<string, number> }) {
  const here = location.hash || "#/";
  // The longest tab the address is in: `#/memory/docs/12` is Docs, not Decisions.
  const current = tabs
    .filter((t) => here === t.hash || here.startsWith(`${t.hash}/`))
    .sort((a, b) => b.hash.length - a.hash.length)[0];
  return (
    <nav className="seg section-tabs" aria-label={label}>
      {tabs.map((t) => {
        const on = t === current;
        const n = counts?.[t.label];
        return (
          <a
            key={t.hash}
            href={t.hash}
            className={`seg-item ${on ? "on" : ""}`}
            aria-current={on ? "page" : undefined}
            onPointerEnter={t.warm}
            onPointerDown={t.warm}
            onClick={(e) => {
              // The section's page already; no history entry for staying put.
              if (on && here === t.hash) e.preventDefault();
            }}
          >
            {t.label}
            {!!n && <span className="count">{n}</span>}
          </a>
        );
      })}
    </nav>
  );
}

/** Memory's tabs: Decisions (and skills, policies, habits, notes), Docs, Wiki. */
export function MemoryTabs() {
  // Docs agents wrote since you last looked: a count on the tab (ADR-0021 §5).
  const unread = useApp((s) => s.unreadDocs);
  return <SectionTabs tabs={MEMORY_TABS} label="Memory" counts={{ Docs: unread }} />;
}

/** History's tabs: past sessions, the away summary, search, the supervisor. */
export function HistoryTabs() {
  return <SectionTabs tabs={HISTORY_TABS} label="History" />;
}
