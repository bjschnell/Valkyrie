// The connection to `valk web`: one WebSocket, requests matched to replies by id,
// pushes handed to a callback. Framework-free on purpose. Phones suspend sockets
// whenever the app is backgrounded, so this spends much of its life reconnecting:
// backoff with jitter, an immediate retry when the page becomes visible or the
// network returns, and a keepalive whose missing reply means a dead socket.
// (Shape borrowed from Alice's socket.ts.)

import { PROTOCOL, type Notice, type Reply, type Request, type ServerMsg } from "../proto";

export type Status = "connecting" | "open" | "reconnecting" | "offline" | "unpaired" | "outdated";

const SUBPROTOCOL = "valk.v1";
const CLOSE_UNPAIRED = 4401;
const KEEPALIVE_MS = 25_000;
const REPLY_TIMEOUT_MS = 10_000;
const BACKOFF_MS = [500, 1_000, 2_000, 4_000, 8_000, 15_000];
const JITTER = 0.2;

type Pending = { resolve: (r: Reply) => void; reject: (e: Error) => void; timer: number };

export interface ConnOptions {
  token: string;
  onPush: (msg: ServerMsg) => void;
  onStatus: (status: Status) => void;
  /** Called on every (re)connect once the daemon answered: resubscribe here. */
  onReady: () => void;
}

export class Conn {
  private ws: WebSocket | null = null;
  private next = 1;
  private pending = new Map<number, Pending>();
  private attempt = 0;
  private timer: number | null = null;
  private keepalive: number | null = null;
  private stopped = false;
  status: Status = "connecting";

  constructor(private opts: ConnOptions) {}

  start(): void {
    window.addEventListener("online", this.retryNow);
    window.addEventListener("offline", this.wentOffline);
    document.addEventListener("visibilitychange", this.onVisible);
    this.connect();
  }

  stop(): void {
    this.stopped = true;
    window.removeEventListener("online", this.retryNow);
    window.removeEventListener("offline", this.wentOffline);
    document.removeEventListener("visibilitychange", this.onVisible);
    this.clearTimers();
    this.ws?.close(1000);
  }

  request(msg: Request): Promise<Reply> {
    return new Promise((resolve, reject) => {
      const ws = this.ws;
      if (!ws || ws.readyState !== WebSocket.OPEN) {
        reject(new Error("not connected"));
        return;
      }
      const req = this.next++;
      const timer = window.setTimeout(() => {
        this.pending.delete(req);
        reject(new Error("no reply"));
      }, REPLY_TIMEOUT_MS);
      this.pending.set(req, { resolve, reject, timer });
      ws.send(JSON.stringify({ ...msg, req }));
    });
  }

  /** Fire-and-forget (keystrokes). False when there is no connection to send on. */
  send(msg: Notice): boolean {
    const ws = this.ws;
    if (!ws || ws.readyState !== WebSocket.OPEN) return false;
    ws.send(JSON.stringify(msg));
    return true;
  }

  private setStatus(status: Status): void {
    if (this.status === status) return;
    this.status = status;
    this.opts.onStatus(status);
  }

  private connect(): void {
    if (this.stopped) return;
    if (!navigator.onLine) {
      this.setStatus("offline");
      return;
    }
    this.setStatus(this.attempt === 0 ? "connecting" : "reconnecting");
    const url = `${location.protocol === "https:" ? "wss" : "ws"}://${location.host}/ws`;
    let ws: WebSocket;
    try {
      ws = new WebSocket(url, [SUBPROTOCOL, `bearer.${this.opts.token}`]);
    } catch {
      this.setStatus("unpaired");
      return;
    }
    this.ws = ws;
    ws.onopen = () => {
      if (ws !== this.ws) return;
      void this.handshake();
    };
    ws.onmessage = (ev) => {
      if (ws !== this.ws) return;
      let msg: ServerMsg;
      try {
        msg = JSON.parse(ev.data as string);
      } catch {
        return;
      }
      if (msg.t === "ok" || msg.t === "err") {
        const p = this.pending.get(msg.req);
        if (!p) return;
        this.pending.delete(msg.req);
        window.clearTimeout(p.timer);
        if (msg.t === "ok") p.resolve(msg.reply);
        else p.reject(new Error(msg.message));
        return;
      }
      this.opts.onPush(msg);
    };
    ws.onclose = (ev) => {
      if (ws !== this.ws) return;
      this.ws = null;
      this.failPending();
      this.clearTimers();
      if (ev.code === CLOSE_UNPAIRED) {
        this.stopped = true;
        this.setStatus("unpaired");
        return;
      }
      this.scheduleRetry();
    };
  }

  private async handshake(): Promise<void> {
    try {
      const hello = await this.request({ t: "hello" });
      if (hello.t !== "hello" || hello.protocol !== PROTOCOL) {
        this.stopped = true;
        this.setStatus("outdated");
        this.ws?.close(1000);
        return;
      }
    } catch {
      this.ws?.close(4000);
      return;
    }
    this.attempt = 0;
    this.setStatus("open");
    this.keepalive = window.setInterval(() => {
      // No reply in time means the socket died without telling us (a phone that
      // slept); closing it starts a reconnect.
      this.request({ t: "hello" }).catch(() => this.ws?.close(4000));
    }, KEEPALIVE_MS);
    this.opts.onReady();
  }

  private scheduleRetry(): void {
    if (this.stopped) return;
    const base = BACKOFF_MS[Math.min(this.attempt, BACKOFF_MS.length - 1)];
    const delay = base * (1 - JITTER + Math.random() * 2 * JITTER);
    this.attempt++;
    this.setStatus(navigator.onLine ? "reconnecting" : "offline");
    this.timer = window.setTimeout(() => this.connect(), delay);
  }

  private retryNow = (): void => {
    if (this.stopped || (this.ws && this.ws.readyState <= WebSocket.OPEN)) return;
    this.clearTimers();
    this.attempt = 0;
    this.connect();
  };

  private wentOffline = (): void => this.setStatus("offline");

  private onVisible = (): void => {
    if (document.visibilityState !== "visible") return;
    // A resumed phone's socket may be dead; the keepalive would find out late.
    if (this.ws?.readyState === WebSocket.OPEN) {
      this.request({ t: "hello" }).catch(() => this.ws?.close(4000));
    } else {
      this.retryNow();
    }
  };

  private failPending(): void {
    for (const p of this.pending.values()) {
      window.clearTimeout(p.timer);
      p.reject(new Error("disconnected"));
    }
    this.pending.clear();
  }

  private clearTimers(): void {
    if (this.timer !== null) window.clearTimeout(this.timer);
    if (this.keepalive !== null) window.clearInterval(this.keepalive);
    this.timer = null;
    this.keepalive = null;
  }
}
