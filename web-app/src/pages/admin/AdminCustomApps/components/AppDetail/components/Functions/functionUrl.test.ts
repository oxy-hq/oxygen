import { describe, expect, it } from "vitest";
import { functionUrl } from "./functionUrl";

describe("functionUrl", () => {
  it("serves a function under the app's own URL as fn/<name>", () => {
    expect(
      functionUrl("https://app.oxygen-hq.com/customer-apps/poke-house/bookkeeping/", "ingest")
    ).toBe("https://app.oxygen-hq.com/customer-apps/poke-house/bookkeeping/fn/ingest");
  });

  it("does not drop the app's last segment when the URL has no trailing slash", () => {
    // `new URL("fn/x", ".../bookkeeping")` would resolve against `.../poke-house/`.
    expect(
      functionUrl("https://app.oxygen-hq.com/customer-apps/poke-house/bookkeeping", "ingest")
    ).toBe("https://app.oxygen-hq.com/customer-apps/poke-house/bookkeeping/fn/ingest");
  });

  it("encodes a name rather than letting it climb out of the app", () => {
    expect(functionUrl("https://h.example/customer-apps/o/a/", "../admin")).toBe(
      "https://h.example/customer-apps/o/a/fn/..%2Fadmin"
    );
  });

  it("names no URL when the app URL does not parse", () => {
    expect(functionUrl("not a url", "ingest")).toBeNull();
  });

  it("names no URL from a bare path — the caller resolves it against an origin first", () => {
    // The registry can answer `url` as `/customer-apps/<org>/<app>/`. The console passes
    // it through `resolveBundleUrl`; a bare path reaching here must not become a URL.
    expect(functionUrl("/customer-apps/local/pulse-demo/", "store")).toBeNull();
  });
});
