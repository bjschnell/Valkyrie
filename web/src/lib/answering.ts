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
}

/** Choices as one string, to tell the prompt answered from the next one. */
export function choicesKey(choices: Choice[]): string {
  return JSON.stringify(choices.map((c) => [c.n ?? null, c.label]));
}

/** What a session's `answering` says about these buttons: null when they're
 * live, else the label tapped. A new prompt on the screen is live again. */
export function answeringLabel(a: Answering | undefined, choices: Choice[]): string | null {
  return a && a.of === choicesKey(choices) ? a.label : null;
}
