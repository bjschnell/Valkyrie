// `valk web`'s own HTTP API (not the daemon's): the device token as a bearer token.

export async function api(token: string, path: string, body?: unknown): Promise<Response> {
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

export async function getJson<T>(token: string, path: string): Promise<T> {
  return (await api(token, path)).json() as Promise<T>;
}

/** Sends a file to this machine, where an agent can read it; its saved path. */
export async function upload(token: string, file: File): Promise<string> {
  const res = await fetch(`/api/upload?name=${encodeURIComponent(file.name)}`, {
    method: "POST",
    headers: { Authorization: `Bearer ${token}`, "Content-Type": "application/octet-stream" },
    body: file,
  });
  if (!res.ok) throw new Error((await res.text()) || `HTTP ${res.status}`);
  return ((await res.json()) as { path: string }).path;
}

export interface Place {
  path: string;
  label: string;
  git: boolean;
}

export interface Places {
  home: string;
  shell: string;
  recent: Place[];
  repos: Place[];
}

export interface Listing {
  path: string;
  label: string;
  parent: string | null;
  git: boolean;
  dirs: Place[];
}

export type Change = "modified" | "added" | "deleted" | "renamed" | "untracked";

export interface FileDiff {
  path: string;
  from?: string;
  status: Change;
  added: number;
  removed: number;
  binary: boolean;
  patch: string;
  cut: boolean;
}

export interface Review {
  root: string;
  branch: string | null;
  files: FileDiff[];
  omitted: number;
}
