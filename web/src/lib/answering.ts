// A prompt answered from the web, shown as sent the moment it's tapped
// (backlog #203): its buttons dim until the daemon confirms the keys arrived.

import type { Choice } from "./choices";

/** One session's answer on its way, or just confirmed. */
export interface Answering {
  /** The choices it answered, as `choicesKey` gives them. */
  of: string;
  /** The label of the choice tapped, lit among the dimmed ones. */
  label: string;
  /** The daemon confirmed it; the screen may still show the old prompt a moment. */
  done: boolean;
  /** The session's status `seq` when it was tapped: once confirmed, a newer one
   * is another prompt, even with the same choices (back-to-back Yes/No). */
  seq: number | null;
}

/** Choices as one string, to tell the prompt answered from the next one. */
export function choicesKey(choices: Choice[]): string {
  return JSON.stringify(choices.map((c) => [c.n ?? null, c.label]));
}

/** What a session's `answering` says about these buttons: null when they're
 * live, else the label tapped. A new prompt on the screen is live again: other
 * choices, or once it's confirmed, a status that moved on (`seq`). */
export function answeringLabel(
  a: Answering | undefined,
  choices: Choice[],
  seq: number | null | undefined,
): string | null {
  if (!a || a.of !== choicesKey(choices)) return null;
  if (a.done && a.seq !== null && seq != null && seq !== a.seq) return null;
  return a.label;
}
