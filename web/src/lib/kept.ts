// What each page last loaded, kept for its next visit so it paints at once and
// refreshes behind it (#104). The hook over this is `useCached` in src/cache.ts.

/** A loader gives the data, null when the daemon couldn't be asked, or throws its reason. */
export type Loader<T> = () => Promise<T | null>;

export class Kept {
  private data = new Map<string, unknown>();
  private going = new Map<string, Promise<unknown>>();

  /** What `key` last loaded, if anything. */
  peek<T>(key: string): T | null {
    return (this.data.get(key) as T | undefined) ?? null;
  }

  /** Loads `key`, one request at a time per key, and keeps what came back.
   * A null or a throw keeps the last data. */
  load<T>(key: string, get: Loader<T>): Promise<T | null> {
    const already = this.going.get(key);
    if (already) return already as Promise<T | null>;
    const p = get()
      .then((v) => {
        if (v !== null) this.data.set(key, v);
        return v;
      })
      .finally(() => this.going.delete(key));
    this.going.set(key, p);
    return p;
  }
}
