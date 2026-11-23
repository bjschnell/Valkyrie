/** What the session page's dock holds under the chat or the terminal. */
export interface DockParts {
  /** The choice buttons of a question on screen. */
  choices: boolean;
  /** Keys a phone keyboard lacks. */
  keybar: boolean;
  /** The message box, its attach, mic and send buttons. */
  composer: boolean;
}

/**
 * At desktop width the terminal takes the keyboard when clicked, so it gets the
 * whole pane: no message box, mic or keys under it, and no choice buttons, whose
 * coming and going would resize the session while you type (a digit answers).
 * On a phone the message box and keys are the terminal's only way in.
 */
export function sessionDock(mode: "chat" | "terminal", desktop: boolean): DockParts {
  const whole = desktop && mode === "terminal";
  return { choices: !whole, keybar: !whole, composer: !whole };
}
