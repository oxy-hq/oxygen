import type { DevicePlatform, NotificationDevice, NotificationLevel } from "@/types/notifications";

/** A device the person has not named: what it is. */
export function fallbackName(platform: DevicePlatform): string {
  if (platform === "ios") return "iOS device";
  if (platform === "android") return "Android device";
  return "Web browser";
}

const SETTINGS_APP: Record<DevicePlatform, string> = {
  ios: "iOS Settings",
  android: "Android Settings",
  web: "browser settings"
};

/**
 * What the device's own settings stop, as it last reported: all of push
 * ("Off in iOS Settings", "Blocked in browser settings"), some kinds, or
 * nothing (`null`).
 */
export function osStatus(device: NotificationDevice): string | null {
  const where = SETTINGS_APP[device.platform] ?? SETTINGS_APP.web;
  if (device.os_permission === "denied") {
    return device.platform === "web" ? "Blocked in browser settings" : `Off in ${where}`;
  }
  const off = device.os_disabled.length;
  if (off > 0) return `${off} ${off === 1 ? "kind" : "kinds"} off in ${where}`;
  return null;
}

/** The floor select's value: `all` for no floor. */
const ALL = "all";

export function floorValue(level: NotificationLevel | null): string {
  return level === null || level === "passive" ? ALL : level;
}

export function floorLevel(value: string): NotificationLevel | null {
  return value === ALL ? null : (value as NotificationLevel);
}
