import { describe, expect, it } from "vitest";
import { auditCredential, hasAuditDetail, tokenIdParam } from "./auditCredential";
import { lifecycleEvent, plainEvent, TOKEN_ID, tokenEvent } from "./auditFixtures";

describe("auditCredential", () => {
  it("is nothing for a row that names no token", () => {
    expect(auditCredential(plainEvent())).toBeNull();
    // An older server sends none of the fields; a newer one may send them all `null`.
    expect(
      auditCredential(
        plainEvent({ token_id: null, token_name: null, token_kind: null, token_prefix: null })
      )
    ).toBeNull();
  });

  it("reads the token as the credential that acted when a key or token performed the row", () => {
    // app.environment.created | actor_type=api_key | token_name="live check"
    const event = tokenEvent();
    expect(event.actor_type).toBe("api_key");
    expect(auditCredential(event)).toEqual({
      role: "acted",
      id: TOKEN_ID,
      name: "live check",
      kind: "Sandbox agent",
      prefix: "oxy_sbx_Ab3x…"
    });
  });

  it("names a lifecycle row's token from its target label, where `token_name` is null", () => {
    // token.created | actor_type=user | token_name=null | token_kind=sandbox_agent
    const event = lifecycleEvent();
    expect(event.token_name).toBeNull();
    expect(event.target_label).toBe("live check");
    expect(auditCredential(event)).toEqual({
      role: "subject",
      id: TOKEN_ID,
      name: "live check",
      kind: "Sandbox agent",
      prefix: "oxy_sbx_Ab3x…"
    });
  });

  it("leaves a lifecycle row's token unnamed when the row has no target label either", () => {
    expect(auditCredential(lifecycleEvent({ target_label: null }))?.name).toBeNull();
  });

  it("never borrows the target's name for a token that acted", () => {
    // The target of an action is the app it touched, not the token that touched it.
    expect(auditCredential(tokenEvent({ token_name: null }))?.name).toBeNull();
  });

  it("names every kind the server sends, and passes on one it does not know", () => {
    const kind = (token_kind: string) => auditCredential(tokenEvent({ token_kind }))?.kind;
    expect(kind("personal")).toBe("Personal");
    expect(kind("legacy_key")).toBe("Legacy API key");
    expect(kind("service_account")).toBe("Service account");
    expect(kind("ci")).toBe("Trusted access");
    expect(kind("sandbox_agent")).toBe("Sandbox agent");
    expect(kind("something_new")).toBe("something_new");
  });

  it("shows a prefix once, however the server ended it", () => {
    const prefix = (token_prefix: string) => auditCredential(tokenEvent({ token_prefix }))?.prefix;
    expect(prefix("oxy_sbx_Ab3x")).toBe("oxy_sbx_Ab3x…");
    expect(prefix("oxy_sbx_Ab3x…")).toBe("oxy_sbx_Ab3x…");
  });
});

describe("hasAuditDetail", () => {
  it("is false for a row with none of the token and client fields", () => {
    expect(hasAuditDetail(plainEvent())).toBe(false);
    expect(hasAuditDetail(plainEvent({ ip: null, user_agent: null, token_id: null }))).toBe(false);
  });

  it("is true for a token, an address or a client alone", () => {
    expect(hasAuditDetail(plainEvent({ token_id: TOKEN_ID }))).toBe(true);
    expect(hasAuditDetail(plainEvent({ ip: "203.0.113.7" }))).toBe(true);
    expect(hasAuditDetail(plainEvent({ user_agent: "oxyc/0.5.2" }))).toBe(true);
  });
});

describe("tokenIdParam", () => {
  it("takes a UUID, in either case", () => {
    expect(tokenIdParam(TOKEN_ID)).toBe(TOKEN_ID);
    expect(tokenIdParam(TOKEN_ID.toUpperCase())).toBe(TOKEN_ID);
  });

  it("reads anything else as no filter, since the route answers 400 to it", () => {
    expect(tokenIdParam(null)).toBeUndefined();
    expect(tokenIdParam("")).toBeUndefined();
    expect(tokenIdParam("refunds task")).toBeUndefined();
    expect(tokenIdParam(`${TOKEN_ID}0`)).toBeUndefined();
  });
});
