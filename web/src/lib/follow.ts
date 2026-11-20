// Beside the Inbox on a wide screen, the right pane shows the top waiting
// session instead of "No session open", and moves to the next one when you
// answer it, unless you're typing there (#248).

/** What the pane does now. */
export type Follow =
  /** Open this session in the pane (replacing the history entry). */
  | { open: number }
  /** Leave it as it is. */
  | null;

export interface FollowState {
  /** The session open in the pane, or null on the bare Inbox. */
  open: number | null;
  /** The session the pane opened by itself, still being followed; null once you picked one. */
  followed: number | null;
  /** Waiting sessions, top first. */
  queue: number[];
  /** You have something typed, or the keyboard, in the open session. */
  typing: boolean;
}

/**
 * Where the pane goes. On the bare Inbox it opens the top waiting session. On
 * a session it opened itself, once that one stops waiting it moves to the next
 * that waits; with nothing else waiting it stays put. A session you opened
 * yourself is never moved away from.
 */
export function follow({ open, followed, queue, typing }: FollowState): Follow {
  if (open === null) return queue.length ? { open: queue[0] } : null;
  if (open !== followed || typing || queue.includes(open)) return null;
  return queue.length ? { open: queue[0] } : null;
}

/** Drafted text in a session's composer, saved by SessionView. */
const draft = (id: number) => {
  try {
    return !!localStorage.getItem(`valk.draft.${id}`);
  } catch {
    return false;
  }
};

/**
 * You're typing in the open session: a field in the pane holds the keyboard
 * with something in it (or is its terminal), or the session has a saved draft.
 */
export function typingIn(id: number, pane: Element | null): boolean {
  if (draft(id)) return true;
  const el = document.activeElement;
  if (!pane || !el || !pane.contains(el)) return false;
  if (el.classList.contains("typing-field")) return true;
  if (el instanceof HTMLTextAreaElement || el instanceof HTMLInputElement) return el.value !== "";
  return (el as HTMLElement).isContentEditable;
}
