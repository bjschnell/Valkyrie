import { useId } from "react";

// The brand, from the reference sheet: a V of two blades, violet and magenta, with a
// green rune diamond held between them; and VALKYRIE, wide and thin, its Y in green.
// Brand colors are fixed, so the mark looks the same under every look.

export function Mark({ size = 24 }: { size?: number }) {
  const id = useId();
  return (
    <svg className="mark" width={size} height={size} viewBox="0 0 64 64" aria-hidden>
      <defs>
        <linearGradient id={`${id}l`} x1="0" y1="0" x2="0.6" y2="1">
          <stop offset="0" stopColor="#6b3aff" />
          <stop offset="1" stopColor="#b06bff" />
        </linearGradient>
        <linearGradient id={`${id}r`} x1="1" y1="0" x2="0.4" y2="1">
          <stop offset="0" stopColor="#ff3ed8" />
          <stop offset="1" stopColor="#8b4bff" />
        </linearGradient>
      </defs>
      <path d="M3 9 L22 14.5 L31.2 43 L31.2 59 Z" fill={`url(#${id}l)`} />
      <path d="M61 9 L42 14.5 L32.8 43 L32.8 59 Z" fill={`url(#${id}r)`} />
      <path d="M32 13 L37 23 L32 33 L27 23 Z" fill="none" stroke="#39ff7a" strokeWidth="2.4" />
      <path d="M32 5 V13 M32 33 V40" stroke="#39ff7a" strokeWidth="1" />
    </svg>
  );
}

export function Wordmark() {
  return (
    <span className="wordmark" role="img" aria-label="Valkyrie">
      <span aria-hidden>
        VALK<span className="wordmark-rune">Y</span>RIE
      </span>
    </span>
  );
}
