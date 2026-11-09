/**
 * The phase table's own invariants, and the parsing that happens before any
 * request. Pure — no server, no credential, no binary.
 *
 * The table IS the runbook, so these are not style checks: a dependency
 * declared out of order makes the printed sequence wrong, and a `manual` row
 * with no reason is how "nobody wrote it" gets laundered into a design
 * decision.
 */

import { describe, expect, it } from "vitest";
import { ExitCode } from "../util/errors.js";
import { assertPhaseTable, PHASE_IDS, PHASES, phasesFor, stages } from "./phases.js";
import { parsePhases, parseTarget, parseWriters } from "./target.js";

describe("the phase table", () => {
  it("holds its own invariants", () => {
    expect(assertPhaseTable()).toEqual([]);
  });

  it("declares every id exactly once, in table order", () => {
    expect(PHASES.map((p) => p.id)).toEqual([...PHASE_IDS]);
  });

  it("gives every manual phase a reason that is not 'nobody wrote it'", () => {
    for (const phase of PHASES.filter((p) => p.automation === "manual")) {
      expect(phase.manualBecause, phase.id).toBeTruthy();
      expect(phase.manualBecause, phase.id).not.toMatch(/nobody|no one|not written/i);
    }
  });

  it("says what a second run does for every phase", () => {
    for (const phase of PHASES) {
      expect([true, false, "conflicts"], phase.id).toContain(phase.idempotent);
    }
  });

  it("leaves the frontline phase out unless the customer has crews", () => {
    expect(phasesFor().map((p) => p.id)).not.toContain("frontline");
    expect(phasesFor({ crews: true }).map((p) => p.id)).toContain("frontline");
  });

  it("filters to the named phases, keeping table order", () => {
    expect(phasesFor({ only: ["airhouse", "org"] }).map((p) => p.id)).toEqual(["org", "airhouse"]);
  });
});

describe("stages", () => {
  /**
   * The sequence crosses the admin/tenant line twice, and that is the fact a
   * standup actually costs: the GitHub import is a tenant act sitting between
   * admin acts, and an assume session closes /admin. If someone reorders the
   * table so this reads 1, the credential dance has either been fixed or the
   * order has been broken — either way this test should be the thing that asks.
   */
  it("groups consecutive phases by surface, and the sequence crosses to tenant twice", () => {
    const grouped = stages(phasesFor({ crews: true }));
    expect(grouped.map((s) => s.surface)).toEqual(["local", "admin", "tenant", "admin", "tenant"]);
    expect(grouped.filter((s) => s.surface === "tenant")).toHaveLength(2);
  });

  it("puts the org before everything that needs its id", () => {
    const ids = phasesFor({ crews: true }).map((p) => p.id);
    for (const phase of PHASES) {
      for (const need of phase.needs) {
        expect(ids.indexOf(need), `${phase.id} needs ${need}`).toBeLessThan(ids.indexOf(phase.id));
      }
    }
  });
});

describe("the target", () => {
  it("defaults the repo, the app and the writer from the slug", () => {
    const target = parseTarget("poke-house", {}, { requireOwner: false });
    expect(target.app).toBe("poke-house");
    expect(target.repo).toMatch(/\/poke-house$/);
    expect(target.writers).toEqual(["app:poke-house"]);
    expect(target.name).toBe("poke-house");
  });

  it("refuses a slug the server would refuse, before any request", () => {
    expect(() => parseTarget("Poke House", {}, { requireOwner: false })).toThrowError(
      /not an organization slug/
    );
  });

  it("asks for an owner only where a write needs one", () => {
    expect(() => parseTarget("acme", {}, { requireOwner: true })).toThrowError(/--owner/);
    expect(parseTarget("acme", {}, { requireOwner: false }).ownerEmail).toBeUndefined();
  });

  it("refuses an owner that cannot be an email", () => {
    expect(() =>
      parseTarget("acme", { owner: "not-an-email" }, { requireOwner: true })
    ).toThrowError(/not an email address/);
  });

  it("carries a usage exit code on every parse refusal", () => {
    try {
      parseTarget("Poke House", {}, { requireOwner: false });
      expect.unreachable("a bad slug must throw");
    } catch (cause) {
      expect((cause as { code?: number }).code).toBe(ExitCode.USAGE);
    }
  });
});

describe("writers", () => {
  it("defaults to the first app's own writer", () => {
    expect(parseWriters([], "store-ops")).toEqual(["app:store-ops"]);
  });

  it("de-duplicates and keeps the typed order", () => {
    expect(parseWriters(["pipeline:toast", "app:a", "pipeline:toast"], "a")).toEqual([
      "pipeline:toast",
      "app:a"
    ]);
  });

  it("refuses anything that is not app: or pipeline:", () => {
    expect(() => parseWriters(["warehouse:toast"], "a")).toThrowError(/app:<slug>/);
    expect(() => parseWriters(["app:"], "a")).toThrowError(/app:<slug>/);
    expect(() => parseWriters(["app:a:b"], "a")).toThrowError(/app:<slug>/);
  });
});

describe("--phase", () => {
  it("accepts an id from the table and de-duplicates", () => {
    expect(parsePhases(["org", "org", "airhouse"])).toEqual(["org", "airhouse"]);
  });

  it("names the whole table when the id is wrong", () => {
    try {
      parsePhases(["warehouse"]);
      expect.unreachable("an unknown phase must throw");
    } catch (cause) {
      const error = cause as { message: string; hint?: string; code?: number };
      expect(error.message).toContain('"warehouse" is not a phase');
      // The hint, not the message: `usageError` keeps "what to type instead"
      // in its own channel so the renderer can set it apart.
      expect(error.hint).toContain("one of: repo, org, billing");
      expect(error.code).toBe(ExitCode.USAGE);
    }
  });
});
