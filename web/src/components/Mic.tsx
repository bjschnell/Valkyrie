import { useEffect, useRef, useState } from "react";
import { canDictate, dictate, join, type Dictation } from "../lib/dictation";
import { toast } from "../store";
import { Mic as MicIcon } from "./icons";

/**
 * A mic for a text field: tap to dictate into it, tap again to stop. What you said
 * goes after what was already there, and stays editable. Hidden where the browser
 * can't listen.
 */
export function MicButton({ value, onChange }: { value: string; onChange: (text: string) => void }) {
  const [on, setOn] = useState(false);
  const session = useRef<Dictation | null>(null);
  const base = useRef("");
  const latest = useRef(onChange);
  latest.current = onChange;

  useEffect(() => () => session.current?.stop(), []);
  if (!canDictate()) return null;

  const toggle = () => {
    if (on) {
      session.current?.stop();
      return;
    }
    base.current = value;
    navigator.vibrate?.(10);
    setOn(true);
    session.current = dictate(
      (settled, forming) => latest.current(join(base.current, join(settled, forming))),
      (error) => {
        setOn(false);
        session.current = null;
        if (error === "not-allowed") toast("Allow the microphone to dictate");
        else if (error) toast(`Dictation stopped: ${error}`);
      },
    );
  };

  return (
    <button
      type="button"
      className={`mic ${on ? "on" : ""}`}
      aria-label={on ? "Stop dictating" : "Dictate"}
      aria-pressed={on}
      onPointerDown={(e) => e.preventDefault()}
      onClick={toggle}
    >
      <MicIcon />
    </button>
  );
}
