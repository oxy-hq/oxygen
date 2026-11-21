// The guard's list is the ui entry's own: every function export of `@oxy-hq/sdk/ui` with a
// PascalCase name, no more and no fewer. A piece added to (or renamed in) the ui entry fails here
// until `SDK_UI_PIECES` names it, so `checkSdkUiCopies` never drifts from what ships.

import { describe, expect, it } from "vitest";
import * as ui from "../ui";
import { SDK_UI_PIECES } from "./sdkUiPieces";

/** A component's name: a capital, then at least one lower-case letter (not `STATUS_TONES`). */
const COMPONENT = /^[A-Z](?=[A-Za-z0-9]*[a-z])[A-Za-z0-9]*$/;

describe("SDK_UI_PIECES", () => {
  it("is exactly the ui entry's components, sorted", () => {
    const exported = Object.entries(ui)
      .filter(([name, value]) => typeof value === "function" && COMPONENT.test(name))
      .map(([name]) => name)
      .sort();
    expect([...SDK_UI_PIECES]).toEqual(exported);
  });

  it("leaves out what is not a component: contexts, constants, helpers", () => {
    for (const name of [
      "TapHintFrame",
      "ModalFrame",
      "STATUS_TONES",
      "MODAL_FOCUSABLE",
      "cx",
      "ui",
      "sortRows"
    ])
      expect(SDK_UI_PIECES, name).not.toContain(name);
  });
});
