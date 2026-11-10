// Oxygen inbox push worker — served as-is at /oxy-push-sw.js, scope "/".
//
// Registered by src/libs/webPush.ts when someone turns on push (Settings →
// Notifications, or the prompt in the inbox). A custom app's own worker
// (/customer-apps/<org>/<app>/__oxy/sw.js) has a narrower scope and its own
// registration, so the two never compete.
//
// A push from a server that has this browser's subscription keys carries a small
// payload, encrypted to this browser and decrypted by it before `push` fires:
//   { v: 1, id, app_name, kind_label, href, thread? }
// ids and config only — who it is from and what kind it is, never what it says
// (that stays in the inbox, behind sign-in; this worker cannot read the app's
// token to fetch it). It shows as "<app_name>" / "<kind_label>", a thread's pushes
// replace each other, and a click opens `href`. A push with no payload (an older
// server, a subscription registered before keys were sent) or a version this
// worker does not know shows the generic line, as before.
// No fetch handler on purpose — this worker never sits between a page and the network.
//
// The message types are mirrored in src/libs/webPush.ts; NOTIFICATIONS_PATH is
// mirrored in crates/api-notifications/src/web_push_payload.rs.

const OPEN_INBOX_MESSAGE = "oxy:open-notifications";
const INBOX_CHANGED_MESSAGE = "oxy:inbox-changed";
const INBOX_URL = "/?notifications=open";
const NOTIFICATIONS_PATH = "/notifications";
const PAYLOAD_VERSION = 1;
// The installable app's icon (public/manifest.webmanifest), so a push reads as Oxygen's.
const ICON_URL = "/icons/icon-192.png";

self.addEventListener("install", () => {
  // Nothing to cache, so nothing for an old version to keep consistent.
  self.skipWaiting();
});

/** Product tabs on this origin — not custom-app tabs, which have no inbox. */
async function productWindows() {
  const windows = await self.clients.matchAll({ type: "window", includeUncontrolled: true });
  return windows.filter((client) => !new URL(client.url).pathname.startsWith("/customer-apps/"));
}

const text = (value) => (typeof value === "string" && value.trim() ? value.trim() : null);

/** A path on this site, or null: never another origin, whatever a payload says. */
function sitePath(value) {
  if (typeof value !== "string" || !value.startsWith("/") || value.startsWith("//")) return null;
  return value.includes("\\") ? null : value;
}

/** The payload this worker knows, or null for a bodiless push or one it does not. */
function readPayload(data) {
  if (!data) return null;
  let payload;
  try {
    payload = data.json();
  } catch {
    return null;
  }
  if (!payload || typeof payload !== "object" || payload.v !== PAYLOAD_VERSION) return null;
  return payload;
}

/** What to show for a push: [title, options]. */
function notificationFor(payload) {
  if (!payload) {
    return [
      "Oxygen",
      {
        body: "You have a new notification",
        icon: ICON_URL,
        tag: "oxy-inbox",
        renotify: true,
        data: { url: INBOX_URL }
      }
    ];
  }
  const thread = text(payload.thread);
  return [
    text(payload.app_name) ?? "Oxygen",
    {
      body: text(payload.kind_label) ?? "You have a new notification",
      icon: ICON_URL,
      // A thread's newest replaces the older; anything else stands on its own.
      tag: thread ?? text(payload.id) ?? "oxy-inbox",
      renotify: true,
      data: { url: sitePath(payload.href) ?? NOTIFICATIONS_PATH, href: true }
    }
  ];
}

self.addEventListener("push", (event) => {
  event.waitUntil(
    (async () => {
      const [title, options] = notificationFor(readPayload(event.data));
      await self.registration.showNotification(title, options);
      // An open tab's badge is now stale; tell it rather than wait for its poll.
      for (const client of await productWindows()) {
        client.postMessage({ type: INBOX_CHANGED_MESSAGE });
      }
    })()
  );
});

self.addEventListener("notificationclick", (event) => {
  event.notification.close();
  const data = event.notification.data ?? {};
  const url = sitePath(data.url) ?? INBOX_URL;
  event.waitUntil(
    (async () => {
      const [existing] = await productWindows();
      if (existing) {
        await existing.focus();
        // With an href the tab goes there; without (a bodiless push) it opens the bell.
        existing.postMessage(
          data.href ? { type: OPEN_INBOX_MESSAGE, href: url } : { type: OPEN_INBOX_MESSAGE }
        );
        return;
      }
      await self.clients.openWindow(url);
    })()
  );
});
