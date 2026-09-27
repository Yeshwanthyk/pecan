/**
 * Getting the user's attention when a background tab needs them: a finished
 * turn or a dialog/ask waiting for an answer. Only fires while the page is
 * hidden; the visible UI already shows both.
 *
 * Channels, each best-effort: title badge (always), a short chime, and a
 * system notification when the user opted in and granted permission. With
 * notifications on, the device also subscribes to web push, so it hears
 * about turns even when the app is closed (the server only pushes devices
 * with no open event stream).
 */
import { api } from "~/api/client";
import { readLocalStorage, writeLocalStorage } from "~/lib/storage";

const NOTIFY_KEY = "pecan:notify";
const SOUND_KEY = "pecan:notify-sound";
const BADGE = /^\(\d+\) /;

let unseen = 0;
let installed = false;
let registration: Promise<ServiceWorkerRegistration | null> = Promise.resolve(null);

export type AttentionKind = "turn-done" | "needs-input";

export function readNotifyEnabled(): boolean {
  return readLocalStorage(NOTIFY_KEY) === "on" && notificationPermission() === "granted";
}

export function readSoundEnabled(): boolean {
  return readLocalStorage(SOUND_KEY) !== "off";
}

export function saveSoundEnabled(enabled: boolean) {
  writeLocalStorage(SOUND_KEY, enabled ? "on" : "off");
}

export function notificationPermission(): NotificationPermission | "unsupported" {
  return typeof Notification === "undefined" ? "unsupported" : Notification.permission;
}

/** Turns system notifications on (asking for permission) or off. Must run
 * from a user gesture for the permission prompt to appear. Resolves to the
 * effective state. */
export async function saveNotifyEnabled(enabled: boolean): Promise<boolean> {
  if (!enabled) {
    writeLocalStorage(NOTIFY_KEY, "off");
    await syncPush(false);
    return false;
  }
  if (typeof Notification === "undefined") return false;
  const permission =
    Notification.permission === "default" ? await Notification.requestPermission() : Notification.permission;
  writeLocalStorage(NOTIFY_KEY, permission === "granted" ? "on" : "off");
  if (permission === "granted") await syncPush(true);
  return permission === "granted";
}

/** Whether this browser can receive web push (iPhone: Home Screen app over HTTPS). */
export function pushSupported(): boolean {
  return "serviceWorker" in navigator && "PushManager" in window && window.isSecureContext;
}

/**
 * Makes this device's push subscription match `enabled`: subscribes with the
 * server's current key (replacing a subscription made with an old key) and
 * registers the endpoint, or unsubscribes. Resolves to whether push is on.
 */
export async function syncPush(enabled: boolean): Promise<boolean> {
  const worker = pushSupported() ? await registration : null;
  if (!worker) return false;
  try {
    const existing = await worker.pushManager.getSubscription();
    if (!enabled) {
      await existing?.unsubscribe();
      await api.pushUnsubscribe();
      return false;
    }
    const key = base64UrlBytes((await api.pushKey()).publicKey);
    let subscription = existing && sameBytes(existing.options.applicationServerKey, key) ? existing : null;
    if (existing && !subscription) await existing.unsubscribe();
    subscription ??= await worker.pushManager.subscribe({ userVisibleOnly: true, applicationServerKey: key });
    await api.pushSubscribe(subscription.endpoint);
    return true;
  } catch {
    // Push is an extra channel; in-page notifications still work without it.
    return false;
  }
}

function base64UrlBytes(text: string): Uint8Array<ArrayBuffer> {
  const binary = atob(text.replace(/-/g, "+").replace(/_/g, "/"));
  return Uint8Array.from(binary, (char) => char.charCodeAt(0));
}

function sameBytes(current: ArrayBuffer | null, expected: Uint8Array): boolean {
  if (!current || current.byteLength !== expected.length) return false;
  const bytes = new Uint8Array(current);
  return bytes.every((byte, index) => byte === expected[index]);
}

/** Registers the service worker (notification clicks, web push) and clears
 * the title badge when the user comes back. Idempotent. */
export function installAttention() {
  if (installed) return;
  installed = true;
  document.addEventListener("visibilitychange", () => {
    if (!document.hidden) clearBadge();
  });
  window.addEventListener("focus", clearBadge);
  if ("serviceWorker" in navigator && window.isSecureContext) {
    registration = navigator.serviceWorker
      .register("/sw.js")
      .then(() => navigator.serviceWorker.ready)
      .catch(() => null);
    // Keep the subscription current (the server key may have changed).
    if (readNotifyEnabled()) void syncPush(true);
  }
}

/** Signals that `sessionId` needs attention. No-op while the page is visible. */
export function notifyAttention(kind: AttentionKind, sessionId: string, title: string | undefined) {
  if (!document.hidden) return;
  unseen += 1;
  document.title = `(${unseen}) ${document.title.replace(BADGE, "")}`;
  if (readSoundEnabled()) chime();
  if (readNotifyEnabled()) {
    const heading = kind === "needs-input" ? "Pi needs your input" : "Pi finished";
    void showNotification(heading, {
      body: title?.trim() || "Session",
      tag: `pecan:${sessionId}`,
      data: { url: `/#/s/${encodeURIComponent(sessionId)}` },
      icon: "/icon-192.png",
    });
  }
}

function clearBadge() {
  unseen = 0;
  document.title = document.title.replace(BADGE, "");
}

async function showNotification(title: string, options: NotificationOptions) {
  // `new Notification` throws on Android Chrome; the service worker path works
  // everywhere notifications do.
  try {
    const registration = await navigator.serviceWorker?.getRegistration();
    if (registration) {
      await registration.showNotification(title, options);
      return;
    }
    new Notification(title, options);
  } catch {
    // Notifications are best-effort; the title badge already signals.
  }
}

let audio: AudioContext | null = null;

function chime() {
  try {
    audio ??= new AudioContext();
    const now = audio.currentTime;
    const gain = audio.createGain();
    gain.gain.setValueAtTime(0.0001, now);
    gain.gain.exponentialRampToValueAtTime(0.08, now + 0.02);
    gain.gain.exponentialRampToValueAtTime(0.0001, now + 0.35);
    gain.connect(audio.destination);
    const tone = audio.createOscillator();
    tone.type = "sine";
    tone.frequency.setValueAtTime(880, now);
    tone.frequency.setValueAtTime(1175, now + 0.12);
    tone.connect(gain);
    tone.start(now);
    tone.stop(now + 0.36);
  } catch {
    // Autoplay policy may block audio until a gesture; ignore.
  }
}
