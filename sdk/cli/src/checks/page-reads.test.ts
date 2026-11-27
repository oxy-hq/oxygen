/**
 * What the checks read from an open page, run against a scripted page
 * (`testing/fake-page.ts`): the functions are the ones the browser runs.
 */

import { describe, expect, it } from "vitest";
import { inFakePage, type PageContent } from "../testing/fake-page.js";
import { fontsLoaded, readLinks, readVisibleText } from "./page-reads.js";

function page<T>(content: PageContent, read: () => T): T {
  return inFakePage(content, read);
}

describe("readLinks", () => {
  it("answers every link target in document order, and no link that downloads", () => {
    const links = page(
      {
        links: [
          "https://app.example.com/a/",
          { href: "https://app.example.com/a/export.csv", download: true },
          "https://app.example.com/a/stores",
          "https://elsewhere.example.com/"
        ]
      },
      () => readLinks(10)
    );
    expect(links).toEqual([
      "https://app.example.com/a/",
      "https://app.example.com/a/stores",
      "https://elsewhere.example.com/"
    ]);
  });

  it("reads at most as many as it is told", () => {
    const links = Array.from({ length: 50 }, (_, i) => `https://app.example.com/a/s${i}`);
    expect(page({ links }, () => readLinks(7))).toHaveLength(7);
  });

  it("answers nothing where there is no page", () => {
    expect(readLinks(10)).toEqual([]);
  });
});

describe("readVisibleText", () => {
  it("answers the page's visible text, blank runs closed up", () => {
    const text = page({ text: "  Stores  \n\n\n\n12 open   \n3 closed\n" }, () =>
      readVisibleText(4_000)
    );
    expect(text).toBe("Stores\n\n12 open\n3 closed");
  });

  it("keeps at most the characters it is told, never half of one", () => {
    const text = page({ text: `${"a".repeat(3_998)}😀😀😀` }, () => readVisibleText(4_000));
    expect(Array.from(text)).toHaveLength(4_000);
    expect(text.endsWith("😀😀")).toBe(true);
    // A page far longer than that is cut the same.
    const long = page({ text: "word ".repeat(100_000) }, () => readVisibleText(4_000));
    expect(Array.from(long)).toHaveLength(4_000);
  });

  it("answers nothing for a page with no body", () => {
    expect(readVisibleText(100)).toBe("");
  });
});

describe("fontsLoaded", () => {
  it("resolves once the page says its fonts are ready, and still resolves where it cannot say", async () => {
    await expect(page({}, () => fontsLoaded())).resolves.toBe(true);
    await expect(fontsLoaded()).resolves.toBe(true);
  });
});
