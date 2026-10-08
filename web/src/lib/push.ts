// Notifications: the service worker, the browser's push subscription, and the
// server's record of it. iOS only offers push to an app added to the Home Screen,
// and every browser only on HTTPS (`tailscale serve`) or localhost.

export type PushState =
  /** Not checked yet. */
  | "unknown"
  /** iOS Safari in a tab: add to the Home Screen first. */
  | "install-first"
  /** Plain http from another machine. */
  | "needs-https"
  | "unsupported"
  | "off"
  | "on"
  | "denied";

const isIos = () =>
  /iPad|iPhone|iPod/.test(navigator.userAgent) || (navigator.platform === "MacIntel" && navigator.maxTouchPoints > 1);

export const isStandalone = () =>
  matchMedia("(display-mode: standalone)").matches ||
  (navigator as Navigator & { standalone?: boolean }).standalone === true;

/** What this browser can do, before asking it anything. */
function support(): PushState | "ok" {
  if (!window.isSecureContext) return "needs-https";
  if (isIos() && !isStandalone()) return "install-first";
  if (!("serviceWorker" in navigator) || !("PushManager" in window) || !("Notification" in window)) {
    return "unsupported";
  }
  return "ok";
}

export function registerWorker(): void {
  if (!("serviceWorker" in navigator) || !window.isSecureContext) return;
  void navigator.serviceWorker.register("/sw.js").catch(() => {});
}

async function api(token: string, path: string, body?: unknown): Promise<Response> {
  const res = await fetch(path, {
    method: body === undefined ? "GET" : "POST",
    headers: {
      Authorization: `Bearer ${token}`,
      ...(body === undefined ? {} : { "Content-Type": "application/json" }),
    },
    body: body === undefined ? undefined : JSON.stringify(body),
  });
  if (!res.ok) throw new Error((await res.text()) || `HTTP ${res.status}`);
  return res;
}

function keyBytes(key: string): Uint8Array<ArrayBuffer> {
  const b64 = key.replace(/-/g, "+").replace(/_/g, "/");
  const raw = atob(b64 + "=".repeat((4 - (b64.length % 4)) % 4));
  return Uint8Array.from(raw, (c) => c.charCodeAt(0));
}

function sameKey(a: ArrayBuffer | null | undefined, b: Uint8Array): boolean {
  if (!a || a.byteLength !== b.byteLength) return false;
  const x = new Uint8Array(a);
  return x.every((v, i) => v === b[i]);
}

async function subscription(): Promise<PushSubscription | null> {
  const reg = await navigator.serviceWorker.ready;
  return reg.pushManager.getSubscription();
}

/**
 * Where notifications stand. When they're on, re-sends the subscription, so the
 * server always has it (after a reinstall of valk, say), and replaces one made
 * for an older server key.
 */
export async function pushState(token: string): Promise<PushState> {
  const s = support();
  if (s !== "ok") return s;
  if (Notification.permission === "denied") return "denied";
  if (Notification.permission !== "granted") return "off";
  const sub = await subscription();
  if (!sub) return "off";
  try {
    const { key } = await (await api(token, "/api/push")).json();
    if (!sameKey(sub.options.applicationServerKey, keyBytes(key))) {
      await sub.unsubscribe();
      return (await enablePush(token)) ? "on" : "off";
    }
    await api(token, "/api/push/subscribe", sub.toJSON());
  } catch {
    /* the server is unreachable; the browser's subscription still stands */
  }
  return "on";
}

/** Asks for permission (call it from a tap) and subscribes. */
export async function enablePush(token: string): Promise<boolean> {
  const permission = await Notification.requestPermission();
  if (permission !== "granted") return false;
  const { key } = await (await api(token, "/api/push")).json();
  const reg = await navigator.serviceWorker.ready;
  const sub =
    (await reg.pushManager.getSubscription()) ??
    (await reg.pushManager.subscribe({ userVisibleOnly: true, applicationServerKey: keyBytes(key) }));
  await api(token, "/api/push/subscribe", sub.toJSON());
  return true;
}

export async function disablePush(token: string): Promise<void> {
  const sub = await subscription();
  if (!sub) return;
  await api(token, "/api/push/unsubscribe", { endpoint: sub.endpoint }).catch(() => {});
  await sub.unsubscribe();
}

export interface Delivery {
  subscribed: number;
  sent: number;
  errors: string[];
}

/** Has the server push this device a notification. */
export async function testPush(token: string): Promise<Delivery> {
  return (await api(token, "/api/push/test", {})).json();
}

/** Notifications already shown are stale once the app itself is on screen. */
export async function clearNotifications(): Promise<void> {
  if (!("serviceWorker" in navigator)) return;
  const reg = await navigator.serviceWorker.getRegistration();
  for (const n of (await reg?.getNotifications()) ?? []) n.close();
}

export function setBadge(count: number): void {
  const nav = navigator as Navigator & {
    setAppBadge?: (n: number) => Promise<void>;
    clearAppBadge?: () => Promise<void>;
  };
  const done = count > 0 ? nav.setAppBadge?.(count) : nav.clearAppBadge?.();
  done?.catch(() => {});
}
