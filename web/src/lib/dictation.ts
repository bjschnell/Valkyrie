// Dictation with the browser's own speech recognition (Chrome, Safari 14.5+). The
// phone keyboard's mic does this too, but not while the key bar or a sheet has the
// focus, and not with a pause-tolerant, keep-going session.

interface Recognition {
  continuous: boolean;
  interimResults: boolean;
  lang: string;
  start(): void;
  stop(): void;
  abort(): void;
  onresult: ((e: { resultIndex: number; results: ArrayLike<ArrayLike<{ transcript: string }> & { isFinal: boolean }> }) => void) | null;
  onerror: ((e: { error: string }) => void) | null;
  onend: (() => void) | null;
}

type RecognitionCtor = new () => Recognition;

function ctor(): RecognitionCtor | null {
  const w = window as unknown as { SpeechRecognition?: RecognitionCtor; webkitSpeechRecognition?: RecognitionCtor };
  return w.SpeechRecognition ?? w.webkitSpeechRecognition ?? null;
}

export function canDictate(): boolean {
  return ctor() !== null && window.isSecureContext;
}

export interface Dictation {
  stop(): void;
}

/**
 * Listens until stopped. `onText` gets everything heard so far: the settled words
 * and the guess still forming. `onEnd` gets an error, if that's why it ended.
 */
export function dictate(onText: (settled: string, forming: string) => void, onEnd: (error?: string) => void): Dictation {
  const Ctor = ctor();
  if (!Ctor) {
    onEnd("no speech recognition in this browser");
    return { stop() {} };
  }
  const rec = new Ctor();
  rec.continuous = true;
  rec.interimResults = true;
  rec.lang = navigator.language || "en-US";
  let settled = "";
  let error: string | undefined;
  rec.onresult = (e) => {
    let forming = "";
    for (let i = e.resultIndex; i < e.results.length; i++) {
      const r = e.results[i];
      if (r.isFinal) settled = join(settled, r[0].transcript);
      else forming = join(forming, r[0].transcript);
    }
    onText(settled, forming);
  };
  rec.onerror = (e) => {
    // Silence ends a session on some browsers; that's not worth a message.
    if (e.error !== "no-speech" && e.error !== "aborted") error = e.error;
  };
  rec.onend = () => onEnd(error);
  rec.start();
  return { stop: () => rec.stop() };
}

/** Joins dictated pieces with one space. */
export function join(a: string, b: string): string {
  const x = a.trimEnd();
  const y = b.trim();
  if (!x) return y;
  if (!y) return x;
  return `${x} ${y}`;
}
