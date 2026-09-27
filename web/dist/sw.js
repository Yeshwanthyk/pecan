// Pecan service worker: notification clicks and (payload-less) web push.
// No fetch handler: the app is served fresh from the local server and must
// never be answered from a stale cache.

self.addEventListener("install", () => self.skipWaiting());
self.addEventListener("activate", (event) => event.waitUntil(self.clients.claim()));

self.addEventListener("notificationclick", (event) => {
  event.notification.close();
  const url = new URL(event.notification.data?.url ?? "/", self.location.origin).href;
  event.waitUntil(
    (async () => {
      const windows = await self.clients.matchAll({ type: "window", includeUncontrolled: true });
      const existing = windows.find((client) => new URL(client.url).origin === self.location.origin);
      if (existing) {
        await existing.focus();
        if ("navigate" in existing) await existing.navigate(url).catch(() => undefined);
        return;
      }
      await self.clients.openWindow(url);
    })(),
  );
});

// Push carries no payload (no encryption needed). Fetch what is pending with
// the device cookie and show it; always show something, as browsers require.
self.addEventListener("push", (event) => {
  event.waitUntil(
    (async () => {
      let items = [];
      try {
        const response = await fetch("/api/push/pending", { credentials: "same-origin" });
        if (response.ok) items = (await response.json()).notifications ?? [];
      } catch {
        // Offline or unpaired: fall through to the generic notice.
      }
      if (items.length === 0) items = [{ title: "Pecan", body: "A session needs attention", url: "/" }];
      await Promise.all(
        items.slice(0, 4).map((item) =>
          self.registration.showNotification(item.title, {
            body: item.body,
            tag: item.tag,
            icon: "/icon-192.png",
            data: { url: item.url ?? "/" },
          }),
        ),
      );
    })(),
  );
});
