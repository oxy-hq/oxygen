// @vitest-environment node
//
// 2.31.1: the shell's tap form for hints renders without a DOM (a screen rendered to markup on a
// server, or in a node test). With no window there is no phone layout (`usePhoneLayout`'s server
// answer), so a tipped `Stat` under it is the desk's, and nothing reads `window` or `document`; a
// phone takes the tap form on its first client render.

import { renderToStaticMarkup } from "react-dom/server";
import { describe, expect, it } from "vitest";
import { TapHintFrame } from "../../ui/containers";
import { Stat } from "../../ui/figures";
import { PhoneTapHintFrame } from "./PhoneTapHintFrame";

describe("PhoneTapHintFrame without a DOM", () => {
  it("renders a tipped Stat to markup in node as the desk draws it", () => {
    expect(typeof (globalThis as { window?: unknown }).window).toBe("undefined");
    const html = renderToStaticMarkup(
      <PhoneTapHintFrame>
        <Stat label='Income' value='$133k' tip='Census ACS, 2023.' boxed />
      </PhoneTapHintFrame>
    );
    expect(html).toContain('aria-label="About Income"');
    expect(html).toContain("oxy-ui-stat__mark");
    expect(html).toContain("oxy-ui-hover-hint");
    expect(html).not.toContain("oxy-phone-hint");
  });

  it("passes an app's own frame on, as it did before the shell had one", () => {
    const html = renderToStaticMarkup(
      <TapHintFrame.Provider value={(name, text) => <i data-app-hint={name}>{text}</i>}>
        <PhoneTapHintFrame>
          <Stat label='Income' value='$133k' tip='Census ACS, 2023.' />
        </PhoneTapHintFrame>
      </TapHintFrame.Provider>
    );
    expect(html).toContain('data-app-hint="About Income"');
    expect(html).not.toContain("oxy-ui-stat__mark");
  });
});
