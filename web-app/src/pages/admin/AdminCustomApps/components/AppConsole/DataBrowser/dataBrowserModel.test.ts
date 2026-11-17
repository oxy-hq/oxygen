import { describe, expect, it } from "vitest";
import {
  absentCopy,
  approxRowsLabel,
  cellView,
  headerTitle,
  narrowedNote,
  nextOrder,
  pageRange,
  pageRows
} from "./dataBrowserModel";

describe("cellView", () => {
  it("draws NULL apart from the string NULL and from an empty string", () => {
    expect(cellView(null, 256)).toEqual({ text: "NULL", isNull: true });
    expect(cellView({ text: "NULL" }, 256)).toEqual({ text: "NULL", isNull: false });
    expect(cellView({ text: "" }, 256)).toEqual({ text: "", isNull: false });
  });

  it("says a value was cut, and how big it was when the server knows", () => {
    expect(cellView({ text: "abc", truncated: true, size: 2048 }, 256)).toEqual({
      text: "abc…",
      isNull: false,
      title: "Cut at 256 characters of 2.0 KiB"
    });
    expect(cellView({ text: "abc", truncated: true }, 256).title).toBe("Cut at 256 characters");
  });

  it("renders JSON as JSON and bytes as hex with their size", () => {
    expect(cellView({ json: { a: [1, null] } }, 256).text).toBe('{"a":[1,null]}');
    expect(cellView({ json: null }, 256)).toEqual({ text: "null", isNull: false });
    expect(cellView({ hex: "cafe", size: 2, truncated: false }, 256)).toEqual({
      text: "\\xcafe",
      isNull: false,
      title: "bytea, 2 B"
    });
    expect(cellView({ hex: "00", size: 4096, truncated: true }, 256).text).toBe("\\x00…");
  });
});

describe("approxRowsLabel", () => {
  it("never reads an unanalyzed table as empty", () => {
    expect(approxRowsLabel(null)).toBe("not analyzed");
    expect(approxRowsLabel(0)).toBe("~0 rows");
    expect(approxRowsLabel(1)).toBe("~1 row");
    expect(approxRowsLabel(12345)).toBe("~12,345 rows");
  });
});

describe("headerTitle", () => {
  it("says whether a column can order the page", () => {
    const base = {
      name: "id",
      type: "bigint",
      kind: "other",
      nullable: false,
      primary_key: true
    } as const;
    expect(headerTitle({ ...base, sortable: true })).toBe(
      "bigint not null, primary key — order by this column"
    );
    expect(
      headerTitle({
        ...base,
        name: "doc",
        type: "json",
        nullable: true,
        primary_key: false,
        sortable: false
      })
    ).toBe("json — Postgres cannot order by this type");
  });
});

describe("nextOrder", () => {
  it("cycles ascending, descending, then the table's own order", () => {
    const asc = nextOrder(undefined, "id");
    expect(asc).toEqual({ column: "id", dir: "asc" });
    const desc = nextOrder(asc, "id");
    expect(desc).toEqual({ column: "id", dir: "desc" });
    expect(nextOrder(desc, "id")).toBeUndefined();
  });

  it("starts another column at ascending", () => {
    expect(nextOrder({ column: "id", dir: "desc" }, "name")).toEqual({
      column: "name",
      dir: "asc"
    });
  });
});

describe("pageRange", () => {
  it("names the rows on screen, one-based", () => {
    expect(pageRange(0, 50)).toBe("Rows 1–50");
    expect(pageRange(50, 7)).toBe("Rows 51–57");
    expect(pageRange(0, 0)).toBe("No rows");
    expect(pageRange(1000, 0)).toBe("No rows past 1,000");
  });
});

describe("pageRows", () => {
  it("numbers rows from the page's offset and keys each cell by its column", () => {
    const rows = pageRows({
      offset: 50,
      columns: [
        {
          name: "id",
          type: "int8",
          kind: "other",
          nullable: false,
          primary_key: true,
          sortable: true
        },
        {
          name: "note",
          type: "text",
          kind: "text",
          nullable: true,
          primary_key: false,
          sortable: true
        }
      ],
      rows: [
        [{ text: "1" }, null],
        [{ text: "2" }, { text: "b" }]
      ]
    });
    expect(rows.map((r) => r.position)).toEqual([51, 52]);
    expect(rows[0].cells).toEqual([
      { column: "id", cell: { text: "1" } },
      { column: "note", cell: null }
    ]);
  });
});

describe("narrowedNote", () => {
  it("speaks only when the server read fewer rows than asked", () => {
    expect(narrowedNote({ limit: 50, requested_limit: 50 })).toBeNull();
    expect(narrowedNote({ limit: 39, requested_limit: 50 })).toBe(
      "39 rows a page: this table is wide, and a page is capped by size."
    );
  });
});

describe("absentCopy", () => {
  it("names the environment and gives the server's reason as a sentence", () => {
    const copy = absentCopy(
      {
        state: "absent",
        code: "no_branch",
        reason: "staging has no OLTP store of its own"
      },
      "Staging"
    );
    expect(copy.title).toBe("No OLTP data to browse in Staging.");
    expect(copy.description).toBe("Staging has no OLTP store of its own.");
    expect(
      absentCopy({ state: "absent", code: "x", reason: "Done." }, "Production").description
    ).toBe("Done.");
  });
});
