// The '?' sheet (#189): the keys that work on this page, then the ones that work
// everywhere. What it says is lib/shortcuts.ts; this is the box.

import { useEffect, useRef } from "react";
import { sections, type PageKey } from "../lib/shortcuts";
import { MAC } from "./Terminal";

export function Shortcuts({ page, onClose }: { page: PageKey; onClose: () => void }) {
  const dialog = useRef<HTMLDialogElement>(null);
  // Opened as a modal, so the rest of the page is inert under it; closed, the
  // keyboard goes back where it was.
  const back = useRef<HTMLElement | null | undefined>(undefined);
  if (back.current === undefined) back.current = document.activeElement as HTMLElement | null;
  useEffect(() => {
    if (!dialog.current?.open) dialog.current?.showModal();
    return () => back.current?.focus?.({ preventScroll: true });
  }, []);
  // Moving away by any route (an Alt chord, the back button) closes it.
  useEffect(() => {
    window.addEventListener("hashchange", onClose);
    return () => window.removeEventListener("hashchange", onClose);
  }, [onClose]);
  // A click that started in the box (selecting its text) isn't one on the backdrop.
  const downOn = useRef<EventTarget | null>(null);

  return (
    <dialog
      ref={dialog}
      className="keys"
      aria-labelledby="keys-title"
      onClose={onClose}
      onPointerDown={(e) => (downOn.current = e.target)}
      onClick={(e) => e.target === dialog.current && downOn.current === dialog.current && onClose()}
    >
      <div className="keys-box">
        <h2 className="keys-title" id="keys-title">
          Keyboard shortcuts
        </h2>
        <div className="keys-body">
          {sections(page, MAC).map((s) => (
            <section key={s.title} className="keys-section">
              <h3>{s.title}</h3>
              <dl>
                {s.shortcuts.map((x) => (
                  <div key={x.does} className="keys-row">
                    <dt>
                      {x.keys.map((combo, i) => (
                        <span key={combo.join("+")} className="keys-combo">
                          {i > 0 && <span className="keys-or">or</span>}
                          {combo.map((k, j) => (
                            <kbd key={j}>{k}</kbd>
                          ))}
                        </span>
                      ))}
                    </dt>
                    <dd>{x.does}</dd>
                  </div>
                ))}
              </dl>
            </section>
          ))}
        </div>
        <div className="keys-foot" aria-hidden>
          <span>
            <kbd>Esc</kbd> or <kbd>?</kbd> close
          </span>
        </div>
      </div>
    </dialog>
  );
}
