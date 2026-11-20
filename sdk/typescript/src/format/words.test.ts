import { describe, expect, it } from "vitest";
import { countPhrase, joinNames, plural, sentenceCase } from "./words";

describe("countPhrase", () => {
  it("spells small counts as words (the reference's cases)", () => {
    expect(countPhrase(0, "store")).toBe("no stores");
    expect(countPhrase(1, "store")).toBe("one store");
    expect(countPhrase(9, "store")).toBe("nine stores");
    expect(countPhrase(10, "store")).toBe("ten stores");
    expect(countPhrase(11, "store")).toBe("11 stores");
    expect(countPhrase(23, "store")).toBe("23 stores");
  });

  it("groups a large count and keeps digits for a negative or a fraction", () => {
    expect(countPhrase(1284, "store")).toBe("1,284 stores");
    expect(countPhrase(-1, "degree")).toBe("-1 degrees");
    expect(countPhrase(2.5, "hour")).toBe("2.5 hours");
    expect(countPhrase(Number.NaN, "store")).toBe("— stores");
  });

  it("takes an irregular plural", () => {
    expect(countPhrase(1, "child", "children")).toBe("one child");
    expect(countPhrase(3, "child", "children")).toBe("three children");
  });
});

describe("plural", () => {
  it("always uses digits, singular only for exactly one", () => {
    expect(plural(0, "document")).toBe("0 documents");
    expect(plural(1, "document")).toBe("1 document");
    expect(plural(1284, "document")).toBe("1,284 documents");
    expect(plural(2, "task was", "tasks were")).toBe("2 tasks were");
  });
});

describe("joinNames", () => {
  it("joins as English, without a serial comma", () => {
    expect(joinNames([])).toBe("");
    expect(joinNames(["Elmwood"])).toBe("Elmwood");
    expect(joinNames(["Elmwood", "Dunmore"])).toBe("Elmwood and Dunmore");
    expect(joinNames(["Elmwood", "Dunmore", "Ironwood"])).toBe("Elmwood, Dunmore and Ironwood");
  });

  it("leaves the names as given, dropping only blank ones", () => {
    expect(joinNames(["Northwind Kitchens - Elmwood", " ", "", "0019 - Birchwood"])).toBe(
      "Northwind Kitchens - Elmwood and 0019 - Birchwood"
    );
  });

  it("takes another conjunction", () => {
    expect(joinNames(["labor", "discounts", "voids"], { conjunction: "or" })).toBe(
      "labor, discounts or voids"
    );
  });
});

describe("sentenceCase", () => {
  it("raises the first letter and leaves the rest alone", () => {
    expect(sentenceCase("three guests")).toBe("Three guests");
    expect(sentenceCase(countPhrase(3, "guest"))).toBe("Three guests");
    expect(sentenceCase("iPhone sales")).toBe("IPhone sales");
    expect(sentenceCase("")).toBe("");
  });

  it("raises a whole code point, not half a surrogate pair", () => {
    expect(sentenceCase("élan")).toBe("Élan");
    expect(sentenceCase("\u{10428}x")).toBe("\u{10400}x");
  });
});
