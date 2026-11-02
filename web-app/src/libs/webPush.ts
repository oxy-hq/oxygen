/**
 * Web Push for the inbox, browser side.
 *
 * A subscription is registered with its endpoint and its encryption keys, so the
 * server can send a small payload encrypted to this browser (RFC 8291): the app's
 * name, the kind's label, where a tap lands — ids and config, never what the
 * notification says. `public/oxy-push-sw.js` shows it as "<app> — <label>". What
 * the notification says stays in the inbox, behind sign-in. A push without a
 * payload (a server from before keys were stored) still reads "You have a new
 * notification".
 *
 * The worker sits at the origin root with scope `/`. A custom app's own worker
 * (`/customer-apps/<org>/<app>/__oxy/sw.js`) has a narrower scope and its own
 * registration, so the two never compete: the longest matching scope controls a
 * page, and a push subscription belongs to one registration.
 */

import type { OsPermission } from "@/types/notifications";

/** Served as-is from `web-app/public`. */
const PUSH_WORKER_URL = "/oxy-push-sw.js";

/** Worker → page: open the inbox (a notification was clicked). Mirrored in the worker. */
export const OPEN_INBOX_MESSAGE = "oxy:open-notifications";

/** Worker → page: a push arrived, so the badge is stale. Mirrored in the worker. */
export const INBOX_CHANGED_MESSAGE = "oxy:inbox-changed";

/** What Settings and the inbox say on an iPhone or iPad in a browser tab. */
export const HOME_SCREEN_HINT =
  "Add Oxygen to your Home Screen (Share → Add to Home Screen) to get notifications here.";

/** Whether this browser can take a push at all. */
export function pushSupported(): boolean {
  return (
    typeof window !== "undefined" &&
    "serviceWorker" in navigator &&
    "PushManager" in window &&
    "Notification" in window
  );
}

/** What `pushNeedsHomeScreen` reads from the browser. */
export interface PushEnvironment {
  userAgent: string;
  platform: string;
  maxTouchPoints: number;
  /** Running as an installed web app (`display-mode: standalone`, or iOS's `navigator.standalone`). */
  standalone: boolean;
}

/** `iPhone OS 17_5 like Mac OS X`, `CPU OS 16_3 like Mac OS X` — not a Mac's `OS X 10_15_7`. */
const IOS_VERSION = /\bOS (\d+)_(\d+)(?:_\d+)? like Mac OS X/;

/**
 * True for an iPhone or iPad browsing in a tab. iOS gives push only to a web app
 * added to the Home Screen (iOS 16.4 and later), so there the way to turn it on
 * is to install Oxygen first. False inside the installed app, on any other
 * device, and on an iOS too old to have web push at all.
 */
export function pushNeedsHomeScreen(env: PushEnvironment): boolean {
  if (env.standalone) return false;
  // iPadOS asks for the desktop site by default, so it reads as a Mac with a touchscreen.
  const iPadAsMac = env.platform === "MacIntel" && env.maxTouchPoints > 1;
  if (!/iPhone|iPad|iPod/.test(env.userAgent) && !iPadAsMac) return false;
  const version = IOS_VERSION.exec(env.userAgent);
  if (!version) return true;
  const [major, minor] = [Number(version[1]), Number(version[2])];
  return major > 16 || (major === 16 && minor >= 4);
}

/** `pushNeedsHomeScreen` for the browser this page is running in. */
export function pushNeedsHomeScreenHere(): boolean {
  if (typeof window === "undefined") return false;
  const nav = navigator as Navigator & { standalone?: boolean };
  return pushNeedsHomeScreen({
    userAgent: nav.userAgent,
    platform: nav.platform,
    maxTouchPoints: nav.maxTouchPoints,
    standalone: nav.standalone === true || window.matchMedia("(display-mode: standalone)").matches
  });
}

/** A VAPID public key (base64url, unpadded) as the bytes `subscribe` wants. */
export function urlBase64ToUint8Array(base64Url: string): Uint8Array<ArrayBuffer> {
  const padding = "=".repeat((4 - (base64Url.length % 4)) % 4);
  const base64 = (base64Url + padding).replace(/-/g, "+").replace(/_/g, "/");
  const raw = atob(base64);
  const bytes = new Uint8Array(raw.length);
  for (let i = 0; i < raw.length; i++) bytes[i] = raw.charCodeAt(i);
  return bytes;
}

function sameKey(current: ArrayBuffer | null, wanted: Uint8Array): boolean {
  if (!current || current.byteLength !== wanted.byteLength) return false;
  const have = new Uint8Array(current);
  return have.every((byte, i) => byte === wanted[i]);
}

/** This browser's permission for notifications, or `unsupported` without the API. */
export function pushPermission(): NotificationPermission | "unsupported" {
  if (typeof window === "undefined" || !("Notification" in window)) return "unsupported";
  return Notification.permission;
}

/**
 * Ask for permission. Call it from the click handler itself, before any `await`:
 * browsers (Safari most strictly) only show the prompt inside the user gesture,
 * and a request that has already crossed an `await` may be refused unseen.
 */
export function requestPushPermission(): Promise<NotificationPermission> {
  return Notification.requestPermission();
}

/** Whether this browser already holds a push subscription from Oxygen's worker. */
export async function isThisDeviceSubscribed(): Promise<boolean> {
  const registration = await navigator.serviceWorker.getRegistration("/");
  if (!registration) return false;
  return (await registration.pushManager.getSubscription()) !== null;
}

/** What the server is told about a subscription: where to push, and how to encrypt. */
export interface ThisDeviceSubscription {
  endpoint: string;
  /** base64url, as `PushSubscription.toJSON()` gives them. */
  keys?: { p256dh: string; auth: string };
}

function describe(subscription: PushSubscription): ThisDeviceSubscription {
  const keys = subscription.toJSON().keys;
  return keys?.p256dh && keys.auth
    ? { endpoint: subscription.endpoint, keys: { p256dh: keys.p256dh, auth: keys.auth } }
    : { endpoint: subscription.endpoint };
}

/**
 * Register the worker and subscribe this browser, once `permission` — the promise
 * `requestPushPermission` returned inside the click — says yes. Returns the
 * endpoint and the keys a payload is encrypted with.
 *
 * A subscription made under a different key (the deployment rotated its VAPID
 * pair) is dropped first: `subscribe` refuses to change keys in place, and a push
 * signed with the new key would be rejected for the old subscription.
 */
export async function subscribeThisDevice(
  publicKey: string,
  permission: Promise<NotificationPermission>
): Promise<ThisDeviceSubscription> {
  if ((await permission) !== "granted") {
    throw new Error("Notifications are blocked for this site in your browser settings.");
  }
  await navigator.serviceWorker.register(PUSH_WORKER_URL, { scope: "/" });
  // `ready` resolves once the worker is active — `subscribe` throws on one that is
  // still installing. Scope `/` covers every product page, so it is ours.
  const registration = await navigator.serviceWorker.ready;
  const key = urlBase64ToUint8Array(publicKey);
  const existing = await registration.pushManager.getSubscription();
  if (existing && sameKey(existing.options.applicationServerKey, key)) return describe(existing);
  if (existing) await existing.unsubscribe();
  const subscription = await registration.pushManager.subscribe({
    userVisibleOnly: true,
    applicationServerKey: key
  });
  return describe(subscription);
}

/**
 * A push's `href` if it is a path on this site, else `null` — what a page will
 * navigate to when the worker says a notification was clicked.
 */
export function pushHref(href: unknown): string | null {
  if (typeof href !== "string" || !href.startsWith("/") || href.startsWith("//")) return null;
  if (href.includes("\\")) return null;
  return href;
}

/**
 * Where this browser keeps the id of the device row it registered, so
 * Settings can say "This device". The device list does not carry the
 * subscription endpoint, so the id the server answered with is the match.
 */
const THIS_DEVICE_KEY = "oxy:push-device-id";

/** The device row this browser registered, if it remembers one. */
export function thisDeviceId(): string | null {
  try {
    return localStorage.getItem(THIS_DEVICE_KEY);
  } catch (err) {
    console.error("Couldn't read this browser's push device", err);
    return null;
  }
}

export function rememberThisDevice(id: string): void {
  try {
    localStorage.setItem(THIS_DEVICE_KEY, id);
  } catch (err) {
    console.error("Couldn't remember this browser's push device", err);
  }
}

/** Forget the remembered device, if it is `id`: it was removed. */
export function forgetThisDevice(id: string): void {
  if (thisDeviceId() !== id) return;
  try {
    localStorage.removeItem(THIS_DEVICE_KEY);
  } catch (err) {
    console.error("Couldn't forget this browser's push device", err);
  }
}

/** What this browser allows, as a device reports it; `null` without the Notification API. */
export function browserPermission(): OsPermission | null {
  if (typeof window === "undefined" || !("Notification" in window)) return null;
  switch (Notification.permission) {
    case "granted":
      return "granted";
    case "denied":
      return "denied";
    default:
      return "unknown";
  }
}

/** First match wins, so Edge (which also says Chrome) comes before Chrome, Chrome before Safari. */
const BROWSERS: [RegExp, string][] = [
  [/Edg\//, "Edge"],
  [/OPR\//, "Opera"],
  [/Firefox\/|FxiOS\//, "Firefox"],
  [/Chrome\/|CriOS\//, "Chrome"],
  [/Safari\//, "Safari"]
];

/** Android says Linux too, and an iPhone says Mac OS X. */
const SYSTEMS: [RegExp, string][] = [
  [/iPhone/, "iPhone"],
  [/iPad/, "iPad"],
  [/Android/, "Android"],
  [/CrOS/, "ChromeOS"],
  [/Mac OS X|Macintosh/, "macOS"],
  [/Windows/, "Windows"],
  [/Linux/, "Linux"]
];

const firstMatch = (table: [RegExp, string][], userAgent: string) =>
  table.find(([pattern]) => pattern.test(userAgent))?.[1] ?? null;

/** "Chrome on macOS": a name for this browser's device row, from its user agent. */
export function browserDeviceName(userAgent: string): string {
  const browser = firstMatch(BROWSERS, userAgent) ?? "Browser";
  const system = firstMatch(SYSTEMS, userAgent);
  return system ? `${browser} on ${system}` : browser;
}
