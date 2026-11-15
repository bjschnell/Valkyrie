// The needs-you count where the OS shows it (#187): the installed app's icon
// badge and the desktop tab's title. The service worker sets the same badge
// from a push while the app is closed; this keeps it right while it's open.

const NAME = "Valkyrie";

/** The tab title: "(3) Valkyrie" with three sessions waiting, plain when none. */
export function titleFor(needs: number): string {
  return needs > 0 ? `(${needs}) ${NAME}` : NAME;
}

/** Show `needs` on the title and, where the browser can, on the app icon. */
export function showNeeds(needs: number): void {
  document.title = titleFor(needs);
  const nav = navigator as Navigator & {
    setAppBadge?: (n?: number) => Promise<void>;
    clearAppBadge?: () => Promise<void>;
  };
  // Not every browser has it, and one that does may refuse outside an installed app.
  try {
    const done = needs > 0 ? nav.setAppBadge?.(needs) : nav.clearAppBadge?.();
    void done?.catch(() => {});
  } catch {
    /* no badge here */
  }
}
