// @vitest-environment jsdom

import { describe, expect, it } from "vitest";
import { appFor } from "./handoff";

const apps = [
  { slug: "ops", url: "/customer-apps/acme/ops/" },
  { slug: "ops-admin", url: "/customer-apps/acme/ops-admin/" }
];

describe("appFor", () => {
  it("finds the app a link opens, deep links included", () => {
    expect(appFor("/customer-apps/acme/ops/orders/42?x=1", apps)?.app.slug).toBe("ops");
    expect(appFor("/customer-apps/acme/ops", apps)?.app.slug).toBe("ops");
  });

  it("does not take one app's path for another's that starts the same", () => {
    expect(appFor("/customer-apps/acme/ops-admin/", apps)?.app.slug).toBe("ops-admin");
  });

  it("leaves every other link alone", () => {
    expect(appFor("#home-today", apps)).toBeNull();
    expect(appFor("/threads/1", apps)).toBeNull();
  });
});
