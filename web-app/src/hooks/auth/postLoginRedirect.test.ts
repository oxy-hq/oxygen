// @vitest-environment jsdom
import { afterEach, describe, expect, it, vi } from "vitest";
import { AuthService } from "@/services/api";
import type { OrgInfo, UserInfo } from "@/types/auth";
import {
  consumeReturnTo,
  handlePostLoginOrgs,
  PENDING_INVITE_TOKEN_KEY,
  resolvePostLoginDestination,
  resolveReturnTo,
  returnToFromUrl,
  sanitizeNextPath,
  stashReturnTo
} from "./postLoginRedirect";

// OAuth login (Google/Okta/GitHub) bounces off-domain and back, so a
// `return_to` must survive the round-trip and be validated before we redirect
// into it. These guard the regression where OAuth dropped `return_to` and sent
// the user to the main product instead of the custom-app subdomain they came
// from.
vi.mock("@/services/api", () => ({
  AuthService: { validateReturnTo: vi.fn() }
}));

const APP_URL = "https://acme--store.customer-apps.example.com/";

describe("postLoginRedirect return_to helpers", () => {
  afterEach(() => {
    sessionStorage.clear();
    vi.clearAllMocks();
  });

  it("stashes a return_to and consumes it exactly once", () => {
    stashReturnTo(APP_URL);
    expect(consumeReturnTo()).toBe(APP_URL);
    // Consumed: a second read is empty (no stale return_to lingers).
    expect(consumeReturnTo()).toBeNull();
  });

  it("clears any prior stash when the current attempt has no return_to", () => {
    // Abandon a login started from a custom app, then start a fresh one with
    // no return_to: the stale destination must not leak into the new attempt.
    stashReturnTo(APP_URL);
    stashReturnTo(undefined);
    expect(consumeReturnTo()).toBeNull();

    stashReturnTo(APP_URL);
    stashReturnTo("");
    expect(consumeReturnTo()).toBeNull();
  });

  it("resolves to the url when the server allows it", async () => {
    vi.mocked(AuthService.validateReturnTo).mockResolvedValue(true);
    await expect(resolveReturnTo(APP_URL)).resolves.toBe(APP_URL);
    expect(AuthService.validateReturnTo).toHaveBeenCalledWith(APP_URL);
  });

  it("resolves to null when the server rejects the url", async () => {
    vi.mocked(AuthService.validateReturnTo).mockResolvedValue(false);
    await expect(resolveReturnTo("https://evil.example.com/")).resolves.toBeNull();
  });

  it("short-circuits to null (no server call) when there is no return_to", async () => {
    await expect(resolveReturnTo(undefined)).resolves.toBeNull();
    await expect(resolveReturnTo(null)).resolves.toBeNull();
    expect(AuthService.validateReturnTo).not.toHaveBeenCalled();
  });

  it("reads the return_to query param from the current login URL", () => {
    window.history.pushState({}, "", "/login?return_to=https%3A%2F%2Fapp.example.com%2Fx");
    expect(returnToFromUrl()).toBe("https://app.example.com/x");

    window.history.pushState({}, "", "/login");
    expect(returnToFromUrl()).toBeNull();
  });
});

describe("handlePostLoginOrgs", () => {
  const user = (standing: Partial<UserInfo> = {}) =>
    ({ is_owner: false, is_app_admin: false, ...standing }) as UserInfo;
  const acme = { id: "org-1", name: "Acme", slug: "acme" } as OrgInfo;

  afterEach(() => sessionStorage.clear());

  it("sends a user with no org to the no-org page", () => {
    expect(handlePostLoginOrgs(user(), [])).toBe("/onboarding");
  });

  it("sends staff standing with no org to the admin console, not an invite wait", () => {
    expect(handlePostLoginOrgs(user({ is_app_admin: true }), [])).toBe("/admin/apps");
  });

  it("sends a member of an org to the dispatcher", () => {
    expect(handlePostLoginOrgs(user({ is_app_admin: true }), [acme])).toBe("/");
  });
});

// `/dev-login?next=…` and `/token-login#next=…` exist so a browser-automation
// run lands on the page under test in one navigation. It must stay a
// same-origin jump: both pages are public and unauthenticated by design, so an
// unsanitized `next` would make either an open redirect that also hands over a
// fresh session.
describe("sanitizeNextPath", () => {
  it("accepts a same-origin path", () => {
    expect(sanitizeNextPath("/ide")).toBe("/ide");
    expect(sanitizeNextPath("/threads/abc?tab=runs")).toBe("/threads/abc?tab=runs");
  });

  it("rejects an absolute URL", () => {
    expect(sanitizeNextPath("https://evil.example.com/")).toBeNull();
  });

  it("rejects a protocol-relative URL", () => {
    expect(sanitizeNextPath("//evil.example.com/")).toBeNull();
  });

  it("rejects a missing or empty value", () => {
    expect(sanitizeNextPath(null)).toBeNull();
    expect(sanitizeNextPath(undefined)).toBeNull();
    expect(sanitizeNextPath("")).toBeNull();
  });

  // `/token-login` follows `next` with a full page load, and a URL parser is
  // more forgiving than a prefix check: it reads `\` as `/` and drops tabs
  // and newlines before it parses. Each of these starts with one slash and
  // resolves to another origin.
  it.each([
    ["a backslash, which a browser reads as a slash", "/\\evil.example.com/"],
    ["a tab before the second slash", "/\t/evil.example.com/"],
    ["a newline before the second slash", "/\n/evil.example.com/"],
    ["a carriage return before a backslash", "/\r\\evil.example.com/"]
  ])("rejects a path that resolves off-origin: %s", (_what, next) => {
    expect(new URL(next, window.location.origin).origin).not.toBe(window.location.origin);
    expect(sanitizeNextPath(next)).toBeNull();
  });

  it("keeps the query and fragment of a path that stays on this origin", () => {
    expect(sanitizeNextPath("/a/store/orders?tab=open#row-3")).toBe(
      "/a/store/orders?tab=open#row-3"
    );
  });
});

// The one destination rule `/dev-login` and `/token-login` share, so a link to
// either lands in the same place.
describe("resolvePostLoginDestination", () => {
  const member = { is_owner: false, is_app_admin: false } as UserInfo;
  const acme = { id: "org-1", name: "Acme", slug: "acme" } as OrgInfo;
  const auth = { user: member, orgs: [acme] };

  afterEach(() => {
    sessionStorage.clear();
    vi.clearAllMocks();
  });

  it("follows a return_to the server allows, over everything else", async () => {
    vi.mocked(AuthService.validateReturnTo).mockResolvedValue(true);
    await expect(
      resolvePostLoginDestination(auth, { returnTo: APP_URL, next: "/ide" })
    ).resolves.toEqual({ kind: "external", url: APP_URL });
  });

  it("falls back to next when the server rejects the return_to", async () => {
    vi.mocked(AuthService.validateReturnTo).mockResolvedValue(false);
    await expect(
      resolvePostLoginDestination(auth, { returnTo: "https://evil.example.com/", next: "/ide" })
    ).resolves.toEqual({ kind: "path", path: "/ide" });
  });

  it("follows a same-origin next without asking the server anything", async () => {
    await expect(resolvePostLoginDestination(auth, { next: "/ide" })).resolves.toEqual({
      kind: "path",
      path: "/ide"
    });
    expect(AuthService.validateReturnTo).not.toHaveBeenCalled();
  });

  it("ignores an off-origin next and lands where the user's orgs say", async () => {
    for (const next of ["https://evil.example.com/", "//evil.example.com/"]) {
      await expect(resolvePostLoginDestination(auth, { next })).resolves.toEqual({
        kind: "path",
        path: "/"
      });
    }
    await expect(
      resolvePostLoginDestination({ user: member, orgs: [] }, { next: null })
    ).resolves.toEqual({ kind: "path", path: "/onboarding" });
  });

  it("leaves a pending invite in place when next wins", async () => {
    // The org dispatcher consumes the invite; a `next` that wins must not.
    sessionStorage.setItem(PENDING_INVITE_TOKEN_KEY, "invite-token");
    await resolvePostLoginDestination(auth, { next: "/ide" });
    expect(sessionStorage.getItem(PENDING_INVITE_TOKEN_KEY)).toBe("invite-token");
  });
});
