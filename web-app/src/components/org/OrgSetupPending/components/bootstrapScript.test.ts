import { describe, expect, it } from "vitest";
import { blankStartCommand, bootstrapScript, shellQuote } from "./bootstrapScript";

const org = { id: "0b7f2c4e", slug: "northwind", name: "Northwind's Shop" };

describe("the oxyc set-up commands", () => {
  it("quotes a name for the shell, apostrophes included", () => {
    expect(shellQuote("Northwind's Shop")).toBe("'Northwind'\\''s Shop'");
  });

  it("creates the repo, pushes it and connects it to this org, in that order", () => {
    const lines = bootstrapScript(org).split("\n");
    const at = (needle: string) => lines.findIndex((l) => l.includes(needle));
    expect(at("oxyc new northwind-oxy --display 'Northwind'\\''s Shop'")).toBeGreaterThan(-1);
    expect(at("git push -u origin main")).toBeGreaterThan(at("oxyc new"));
    expect(at("orgs/0b7f2c4e/github/namespaces")).toBeGreaterThan(at("git push"));
    expect(at("-X POST orgs/0b7f2c4e/onboarding/github")).toBeGreaterThan(
      at("github/repositories")
    );
  });

  it("starts a blank one with a single API call", () => {
    expect(blankStartCommand(org)).toBe(
      "oxyc api -X POST orgs/0b7f2c4e/onboarding/new -f name='Northwind'\\''s Shop'"
    );
  });
});
