// @vitest-environment jsdom
//
// downloadCsv hands the browser a file: a Blob of `toCsv`'s document behind the UTF-8 byte-order
// mark Excel needs, typed text/csv, named with .csv, clicked through a link that is gone again
// at once, its object URL revoked a second later (not in the same tick, which cancels Safari's
// download).

import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { toCsv } from "../../format";
import { downloadCsv } from "./downloadCsv";

interface Store {
  name: string;
  sales: number | null;
  note: string;
}
const ROWS: Store[] = [
  { name: "Café Ñandú", sales: 1234.5, note: 'Said "hi", left' },
  { name: "=HYPERLINK()", sales: null, note: "" }
];
const COLUMNS = [
  { header: "Store", value: "name" as const },
  { header: "Sales", value: (s: Store) => s.sales },
  { header: "Note", value: "note" as const }
];

let blobs: Blob[];
let clicked: { download: string; href: string; connected: boolean }[];
let revoked: string[];

beforeEach(() => {
  blobs = [];
  clicked = [];
  revoked = [];
  Object.assign(URL, {
    createObjectURL: (b: Blob) => {
      blobs.push(b);
      return `blob:test/${blobs.length}`;
    },
    revokeObjectURL: (url: string) => revoked.push(url)
  });
  vi.spyOn(HTMLAnchorElement.prototype, "click").mockImplementation(function (
    this: HTMLAnchorElement
  ) {
    clicked.push({ download: this.download, href: this.href, connected: this.isConnected });
  });
});

afterEach(() => {
  vi.useRealTimers();
  vi.restoreAllMocks();
  document.body.innerHTML = "";
});

/** A Blob's bytes as UTF-8 text with the BOM kept (a default decode drops it). */
function bytesOf(blob: Blob): Promise<string> {
  return new Promise((resolve, reject) => {
    const reader = new FileReader();
    reader.onload = () =>
      resolve(new TextDecoder("utf-8", { ignoreBOM: true }).decode(reader.result as ArrayBuffer));
    reader.onerror = () => reject(reader.error);
    reader.readAsArrayBuffer(blob);
  });
}

describe("downloadCsv", () => {
  it("downloads toCsv's document behind the UTF-8 byte-order mark, as text/csv", async () => {
    downloadCsv(ROWS, COLUMNS, "stores.csv");
    expect(blobs).toHaveLength(1);
    expect(blobs[0].type).toBe("text/csv;charset=utf-8");
    const text = await bytesOf(blobs[0]);
    expect(text.charCodeAt(0)).toBe(0xfeff);
    expect(text.slice(1)).toBe(toCsv(ROWS, COLUMNS));
    // toCsv's quoting and formula guard, carried through unchanged.
    expect(text.slice(1)).toBe(
      'Store,Sales,Note\r\nCafé Ñandú,1234.5,"Said ""hi"", left"\r\n\'=HYPERLINK(),,'
    );
  });

  it("clicks a link named for the file, in the page only for the click", () => {
    downloadCsv(ROWS, COLUMNS, "stores");
    expect(clicked).toEqual([{ download: "stores.csv", href: "blob:test/1", connected: true }]);
    expect(document.querySelector("a")).toBeNull();
    downloadCsv(ROWS, COLUMNS, "Q3.CSV");
    expect(clicked[1].download).toBe("Q3.CSV");
  });

  it("revokes the object URL a second later, not in the same tick", () => {
    vi.useFakeTimers();
    downloadCsv(ROWS, COLUMNS, "stores.csv");
    expect(revoked).toEqual([]);
    vi.advanceTimersByTime(999);
    expect(revoked).toEqual([]);
    vi.advanceTimersByTime(1);
    expect(revoked).toEqual(["blob:test/1"]);
  });
});
