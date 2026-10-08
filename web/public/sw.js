// Valkyrie's service worker: it shows pushes from `valk web` and, when one is
// tapped, opens the session it's about. It caches nothing: the app is no use
// without the live daemon, and a stale copy would only confuse.

self.addEventListener("install", () => self.skipWaiting());
self.addEventListener("activate", (event) => event.waitUntil(self.clients.claim()));

self.addEventListener("push", (event) => {
  let note;
  try {
    note = event.data ? event.data.json() : {};
  } catch {
    note = { title: "Valkyrie", body: event.data ? event.data.text() : "" };
  }
  const url = note.session != null ? `/#/s/${note.session}` : "/";
  event.waitUntil(
    (async () => {
      if (typeof note.badge === "number" && navigator.setAppBadge) {
        try {
          if (note.badge > 0) await navigator.setAppBadge(note.badge);
          else await navigator.clearAppBadge();
        } catch {
          /* no badge here */
        }
      }
      // Every push must show something (iOS revokes silent ones).
      await self.registration.showNotification(note.title || "Valkyrie", {
        body: note.body || "",
        tag: note.tag || "valk",
        renotify: true,
        icon: "/icons/icon-192.png",
        badge: "/icons/badge-96.png",
        timestamp: Date.now(),
        data: { url, session: note.session ?? null, seq: note.seq ?? null },
      });
    })(),
  );
});

self.addEventListener("notificationclick", (event) => {
  event.notification.close();
  const url = (event.notification.data && event.notification.data.url) || "/";
  event.waitUntil(
    (async () => {
      const windows = await self.clients.matchAll({ type: "window", includeUncontrolled: true });
      const open = windows.find((w) => new URL(w.url).origin === self.location.origin);
      if (open) {
        await open.focus();
        open.postMessage({ t: "open", url });
      } else {
        await self.clients.openWindow(url);
      }
    })(),
  );
});
