import { describe, expect, it } from "vitest";
import { possessive } from "./possessive";

describe("possessive", () => {
  it("adds 's, or only ' after an s", () => {
    expect(possessive("Northwind")).toBe("Northwind's");
    expect(possessive("Northwind Traders")).toBe("Northwind Traders'");
  });
});
