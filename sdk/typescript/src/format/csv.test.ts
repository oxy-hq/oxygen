import { describe, expect, it } from "vitest";
import { csvCell, toCsv } from "./csv";

interface Store {
  name: string;
  sales: number | null;
  open: boolean;
}

const STORES: Store[] = [
  { name: "Alder Creek", sales: 1234.5, open: true },
  { name: "Birchwood, East", sales: null, open: false }
];

describe("toCsv", () => {
  it("writes a header row, then a row per record, from keys and readers", () => {
    const csv = toCsv(STORES, [
      { header: "Store", value: "name" },
      { header: "Sales", value: (s) => s.sales },
      { header: "Open", value: "open" },
      { header: "Row", value: (_s, index) => index + 1 }
    ]);
    expect(csv).toBe(
      'Store,Sales,Open,Row\r\nAlder Creek,1234.5,true,1\r\n"Birchwood, East",,false,2'
    );
  });

  it("ends lines with CRLF and adds none after the last", () => {
    const csv = toCsv(STORES, [{ header: "Store", value: "name" }]);
    expect(csv.split("\r\n")).toHaveLength(3);
    expect(csv.endsWith("\n")).toBe(false);
    expect(csv.replace(/\r\n/g, "")).not.toMatch(/[\r\n]/);
  });

  it("gives the header alone for no rows, and escapes the header too", () => {
    expect(toCsv<Store>([], [{ header: "Sales, net", value: "sales" }])).toBe('"Sales, net"');
  });

  it("writes a key's Date as ISO and an invalid one as empty", () => {
    const rows = [{ at: new Date("2026-11-19T12:00:00Z") }, { at: new Date(Number.NaN) }];
    expect(toCsv(rows, [{ header: "At", value: "at" }])).toBe("At\r\n2026-11-19T12:00:00.000Z\r\n");
  });
});

describe("csvCell quoting", () => {
  it("quotes a comma, a quote (doubled), a newline or a carriage return", () => {
    expect(csvCell("a,b")).toBe('"a,b"');
    expect(csvCell('Say "hi"')).toBe('"Say ""hi"""');
    expect(csvCell("line one\nline two")).toBe('"line one\nline two"');
    expect(csvCell("a\rb")).toBe('"a\rb"');
    expect(csvCell("plain")).toBe("plain");
  });

  it("quotes leading or trailing spaces so they survive", () => {
    expect(csvCell(" Rivermark")).toBe('" Rivermark"');
    expect(csvCell("Rivermark ")).toBe('"Rivermark "');
  });
});

describe("csvCell formula injection", () => {
  it("puts an apostrophe before a text field a spreadsheet would run", () => {
    expect(csvCell("=SUM(A1:A9)")).toBe("'=SUM(A1:A9)");
    expect(csvCell("+1")).toBe("'+1");
    expect(csvCell("-cmd|' /C calc'!A0")).toBe("'-cmd|' /C calc'!A0");
    expect(csvCell("@SUM(1)")).toBe("'@SUM(1)");
    expect(csvCell("\t=1")).toBe("'\t=1");
    expect(csvCell("-1e3")).toBe("'-1e3");
  });

  it("escapes and quotes when both apply", () => {
    expect(csvCell('=HYPERLINK("http://x","y")')).toBe('"\'=HYPERLINK(""http://x"",""y"")"');
  });

  it("leaves numbers, and a string that is only a decimal number, alone", () => {
    expect(csvCell(-5)).toBe("-5");
    expect(csvCell("-4")).toBe("-4");
    expect(csvCell("-0.5")).toBe("-0.5");
    expect(csvCell("12")).toBe("12");
  });
});

describe("csvCell values", () => {
  it("writes a missing value as an empty field, never NaN or null", () => {
    for (const value of [null, undefined, Number.NaN, Number.POSITIVE_INFINITY]) {
      expect(csvCell(value)).toBe("");
    }
  });

  it("writes booleans and bigints as their text", () => {
    expect(csvCell(true)).toBe("true");
    expect(csvCell(12345678901234567890n)).toBe("12345678901234567890");
  });
});

describe("toCsv: values that are not cells", () => {
  it("writes an object or array as JSON, never [object Object]", () => {
    const rows = [{ tags: { a: 1 }, list: [1, 2] }];
    const out = toCsv(rows, [
      { header: "Tags", value: "tags" },
      { header: "List", value: "list" }
    ]);
    expect(out).toBe('Tags,List\r\n"{""a"":1}","[1,2]"');
  });
});
