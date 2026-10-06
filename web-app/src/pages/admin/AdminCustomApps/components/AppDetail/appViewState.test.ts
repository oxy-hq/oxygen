import { describe, expect, it } from "vitest";
import {
  fromPreviewPath,
  readAppViewState,
  toPreviewPath,
  writeAppViewState
} from "./appViewState";

const params = (s: string) => new URLSearchParams(s);
const published = { channel: "published" as const };
const draftOnly = { channel: "draft" as const };

describe("readAppViewState", () => {
  it("falls back on anything the UI cannot render", () => {
    // A URL is user input. `?device=phone` should show the desktop preview,
    // not take the page down.
    // `section=telemetry` is deliberately a name no section has ever had —
    // this assertion used `logs` until the Logs section shipped and quietly
    // started passing for the wrong reason.
    const v = readAppViewState(params("device=phone&channel=nightly&section=telemetry"), published);
    expect(v.device).toBe("desktop");
    expect(v.channel).toBe("published");
    expect(v.section).toBeNull();
  });

  it("reads every section the dossier has", () => {
    // `SECTIONS` is a runtime list beside a type; nothing ties the two
    // together, so a section added to the union and forgotten here type-checks
    // and then makes its own deep link silently open nothing.
    for (const section of [
      "status",
      "builds",
      "access",
      "functions",
      "secrets",
      "availability",
      "issues",
      "logs",
      "activity",
      "settings"
    ]) {
      expect(readAppViewState(params(`section=${section}`), published).section).toBe(section);
    }
  });

  it("takes the channel default from the app, not from a constant", () => {
    // An app with nothing published must open on Draft — otherwise the toolbar
    // selects a disabled option and the iframe requests a bundle that does not
    // exist, which hangs the preview rather than erroring.
    expect(readAppViewState(params(""), draftOnly).channel).toBe("draft");
    expect(readAppViewState(params(""), published).channel).toBe("published");
    // An explicit param still wins over either default.
    expect(readAppViewState(params("channel=published"), draftOnly).channel).toBe("published");
  });

  it("refuses a preview path that is not inside this app", () => {
    // The param points a same-origin frame. An absolute URL — or a
    // protocol-relative one, which is the version that looks like a path —
    // would let a shared admin link aim that frame at another origin.
    expect(
      readAppViewState(params("preview=https://evil.example.com/"), published).preview
    ).toBeNull();
    expect(readAppViewState(params("preview=//evil.example.com/"), published).preview).toBeNull();
    expect(readAppViewState(params("preview=stores"), published).preview).toBeNull();
    expect(readAppViewState(params("preview=/stores?limit=20"), published).preview).toBe(
      "/stores?limit=20"
    );
  });
});

describe("writeAppViewState", () => {
  it("drops a param that is back at its default", () => {
    // Otherwise every visit accumulates `?device=desktop&channel=published` and
    // the URL an operator copies is noise around the one thing they changed.
    const out = writeAppViewState(params("device=mobile"), { device: "desktop" }, published);
    expect(out.get("device")).toBeNull();
  });

  it("measures the channel default per app", () => {
    // `channel=draft` is the default for an unpublished app, so it is noise
    // there and meaningful on a published one.
    expect(
      writeAppViewState(params(""), { channel: "draft" }, draftOnly).get("channel")
    ).toBeNull();
    expect(writeAppViewState(params(""), { channel: "draft" }, published).get("channel")).toBe(
      "draft"
    );
  });

  it("leaves params it does not own alone", () => {
    // The apps table owns filter/sort/group on this same query string. Leaving
    // the detail has to return to the list the operator left, so a view change
    // must not clear them.
    const out = writeAppViewState(
      params("q=book&sort=name&group=org"),
      { device: "mobile" },
      published
    );
    expect(out.get("q")).toBe("book");
    expect(out.get("sort")).toBe("name");
    expect(out.get("group")).toBe("org");
    expect(out.get("device")).toBe("mobile");
  });

  it("treats the app root as no preview state", () => {
    expect(writeAppViewState(params(""), { preview: "/" }, published).get("preview")).toBeNull();
    expect(writeAppViewState(params(""), { preview: "/stores" }, published).get("preview")).toBe(
      "/stores"
    );
  });

  it("round-trips through the reader", () => {
    const written = writeAppViewState(
      params(""),
      {
        device: "mobile",
        channel: "draft",
        section: "builds",
        fn: "syncOrders",
        preview: "/x?a=1"
      },
      published
    );
    expect(readAppViewState(written, published)).toEqual({
      device: "mobile",
      channel: "draft",
      section: "builds",
      fn: "syncOrders",
      preview: "/x?a=1"
    });
  });
});

describe("preview path <-> URL", () => {
  const base = "/customer-apps/poke-house/bookkeeping/";

  it("stores a location inside the app as a path", () => {
    expect(
      toPreviewPath(
        "https://app.oxygen-hq.com/customer-apps/poke-house/bookkeeping/?vendor=ubereats&month=2026-07",
        base
      )
    ).toBe("/?vendor=ubereats&month=2026-07");
  });

  it("refuses a location outside the app", () => {
    // The preview navigated somewhere the admin URL must not be able to
    // reproduce — a link that pointed the frame there would be the admin
    // console lending its same-origin frame to another destination.
    expect(toPreviewPath("https://app.oxygen-hq.com/admin/apps", base)).toBeNull();
    expect(
      toPreviewPath("https://app.oxygen-hq.com/customer-apps/other-org/other-app/", base)
    ).toBeNull();
  });

  it("round-trips", () => {
    const origin = "https://app.oxygen-hq.com";
    const path = "/stores/20?limit=20";
    const url = fromPreviewPath(path, base, origin);
    expect(url).not.toBeNull();
    expect(toPreviewPath(url as string, base)).toBe(path);
  });

  it("refuses a path that climbs out of the bundle prefix", () => {
    // `/../../../admin/apps` passes the reader's absolute and
    // protocol-relative checks — it starts with a single slash — and `URL`
    // then normalises it clean out of the app. Same-origin, so the blast
    // radius is small, but the module's claim is that a link cannot aim this
    // frame somewhere else, and this would.
    const origin = "https://app.oxygen-hq.com";
    expect(readAppViewState(params("preview=/../../../admin/apps"), published).preview).toBe(
      "/../../../admin/apps"
    );
    expect(fromPreviewPath("/../../../admin/apps", base, origin)).toBeNull();

    // …and the percent-encoded form is NOT the same attack, so it is not
    // refused: `%2f` is a literal character in a path segment, not a
    // separator, so `..%2f..%2fadmin` stays inside the bundle prefix as one
    // oddly-named segment. Pinned because "reject anything containing .." is
    // the tempting over-fix, and it would break a legitimate path.
    expect(fromPreviewPath("/..%2f..%2fadmin", base, origin)).toContain(
      "/customer-apps/poke-house/bookkeeping/"
    );
  });
});
