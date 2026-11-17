import { describe, expect, it } from "vitest";

import { ExitCode } from "../util/errors.js";
import { blockersOf, productionRefused, remedyFor } from "./production-refusal.js";

const REFUSED = JSON.stringify({
  code: "required_secrets_unset",
  error: "required_secrets_unset",
  message: "production refused this promoting publish, so nothing of it was published",
  blockers: [
    {
      code: "required_secrets_unset",
      message: "the build declares required keys production has no value for: STRIPE_KEY"
    },
    {
      code: "schedule_removal_unconfirmed",
      message: "this retires the schedule of nightly: name each to confirm"
    },
    {
      code: "same_person_promote",
      message: "this app requires a second person to promote"
    }
  ]
});

describe("a refused promoting publish", () => {
  it("is read from the body a refused promote answers, and from nothing else", () => {
    expect(blockersOf(REFUSED)?.map((b) => b.code)).toEqual([
      "required_secrets_unset",
      "schedule_removal_unconfirmed",
      "same_person_promote"
    ]);
    expect(blockersOf("not an app admin")).toBeUndefined();
    expect(blockersOf(JSON.stringify({ code: "sandbox_token_refused" }))).toBeUndefined();
    expect(blockersOf(JSON.stringify({ blockers: [] }))).toBeUndefined();
    expect(blockersOf(JSON.stringify({ blockers: [{ code: 7 }] }))).toBeUndefined();
  });

  it("names every blocker, exits as a refused promote does, and says what to do", () => {
    const error = productionRefused(409, REFUSED, "acme/sales");
    expect(error?.code).toBe(ExitCode.REQUEST);
    expect(error?.message).toBe(
      "production refused this promoting publish (409): required_secrets_unset, schedule_removal_unconfirmed, same_person_promote"
    );
    expect(error?.detail).toContain("required_secrets_unset: the build declares");
    expect(error?.detail).toContain("STRIPE_KEY");
    expect(error?.serverCode).toBe("required_secrets_unset");
    const hint = error?.hint ?? "";
    expect(hint).toContain("Secrets panel, on Production");
    expect(hint).toContain("--remove-schedule <function>");
    expect(hint).toContain(
      "publish without --promote, then someone else runs `oxyc promote acme/sales`"
    );
    expect(hint).toContain("nothing was published, staging included");
  });

  it("leaves every other refusal to the generic path", () => {
    expect(productionRefused(403, '{"error":"not an app admin"}', "acme/sales")).toBeUndefined();
  });

  it("has a remedy for each blocker a promoting publish can meet", () => {
    for (const code of [
      "required_secrets_unset",
      "schedule_removal_unconfirmed",
      "same_person_promote",
      "validation_not_passed",
      "staging_never_served"
    ]) {
      expect(remedyFor(code, "acme/sales"), code).toBeTruthy();
    }
    expect(remedyFor("something_new", "acme/sales")).toBeUndefined();
  });
});
