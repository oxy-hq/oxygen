// @vitest-environment jsdom

import { afterEach, describe, expect, it } from "vitest";
import {
  forgetKioskBrowser,
  hasKioskHintCookie,
  isRememberedKioskBrowser,
  mayBeKioskBrowser,
  rememberKioskBrowser
} from "./kioskBrowser";

const setCookie = (cookie: string) => {
  // biome-ignore lint/suspicious/noDocumentCookie: jsdom has no Cookie Store API; this stands in for the server's Set-Cookie
  document.cookie = `${cookie}; Path=/`;
};
const clearCookie = (name: string) => setCookie(`${name}=; Max-Age=0`);

afterEach(() => {
  localStorage.clear();
  for (const name of ["oxy_kiosk_hint", "oxy_kiosk_hint_extra", "other"]) {
    clearCookie(name);
  }
});

describe("hasKioskHintCookie", () => {
  it("reads the hint among other cookies", () => {
    setCookie("other=x");
    setCookie("oxy_kiosk_hint=1");
    expect(hasKioskHintCookie()).toBe(true);
  });

  it("is false with no hint, a cleared one, or a cookie that only starts with its name", () => {
    expect(hasKioskHintCookie()).toBe(false);
    setCookie("oxy_kiosk_hint_extra=1");
    expect(hasKioskHintCookie()).toBe(false);
    setCookie("oxy_kiosk_hint=");
    expect(hasKioskHintCookie()).toBe(false);
  });
});

describe("the kiosk memory", () => {
  it("is set by remember and cleared only by forget", () => {
    rememberKioskBrowser();
    expect(isRememberedKioskBrowser()).toBe(true);
    forgetKioskBrowser();
    expect(isRememberedKioskBrowser()).toBe(false);
  });
});

describe("mayBeKioskBrowser", () => {
  it("is true on either signal and false on neither", () => {
    expect(mayBeKioskBrowser()).toBe(false);
    setCookie("oxy_kiosk_hint=1");
    expect(mayBeKioskBrowser()).toBe(true);
    clearCookie("oxy_kiosk_hint");
    rememberKioskBrowser();
    expect(mayBeKioskBrowser()).toBe(true);
  });
});
