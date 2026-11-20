// @vitest-environment node
//
// Every contrast case of the desk UI, in every direction theme.css ships, light and dark: the
// recipes' (contrast.cases.ts) and each group's (<group>/contrast.cases.ts). WCAG 2.2 ratio, the
// theme's own maths (src/theme/contrast-math.ts); a case that names a token theme.css does not
// define fails as "a colour is missing".

import { describe, expect, it } from "vitest";
import { contrastCases as actions } from "./actions/contrast.cases";
import { contrastCases as charts } from "./charts/contrast.cases";
import { contrastCases as containers } from "./containers/contrast.cases";
import { recipeCases } from "./contrast.cases";
import {
  caseLabel,
  measureUi,
  UI_THEMES,
  type UiContrastCase,
  type UiMode
} from "./contrast.harness";
import { contrastCases as feedback } from "./feedback/contrast.cases";
import { contrastCases as figures } from "./figures/contrast.cases";
import { contrastCases as focus } from "./focus/contrast.cases";
import { contrastCases as forms } from "./forms/contrast.cases";
import { contrastCases as navigation } from "./navigation/contrast.cases";
import { contrastCases as patterns } from "./patterns/contrast.cases";
import { contrastCases as tables } from "./tables/contrast.cases";

/** Each source, by the file that owns it. */
const SOURCES: Record<string, UiContrastCase[]> = {
  recipes: recipeCases,
  focus,
  actions,
  navigation,
  figures,
  tables,
  charts,
  containers,
  forms,
  feedback,
  patterns
};

/**
 * Cases a direction misses today, as `caseLabel` writes them. A miss is a design finding: it is
 * recorded here and reported, not hidden; each runs as `it.fails`, so the suite stays green and
 * the entry must go the moment the tokens or the piece are fixed.
 *
 * Empty since decision 64 (20 Nov): the four status colours that failed on a selected table row
 * (light success and warning series, the dark info series, the dark info chip's word) took the
 * smallest lightness change that passes (the theme's reference tokens say each one's reading).
 */
const KNOWN_FAILURES: readonly string[] = [];

const MODES: UiMode[] = ["light", "dark"];

describe("ui contrast cases", () => {
  it("are named once each, with a floor of at least 2:1", () => {
    const all = Object.values(SOURCES).flat();
    const names = all.map((c) => c.name);
    expect(new Set(names).size, "a case name repeats").toBe(names.length);
    for (const c of all) expect(c.min, c.name).toBeGreaterThanOrEqual(2);
    expect(recipeCases.length).toBeGreaterThan(40);
  });
});

for (const [source, cases] of Object.entries(SOURCES)) {
  for (const id of UI_THEMES) {
    for (const mode of MODES) {
      const mine = cases.filter((c) => !c.modes || c.modes.includes(mode));
      if (mine.length === 0) continue;
      describe(`${source}: ${id} ${mode}`, () => {
        for (const c of mine) {
          const label = caseLabel(id, mode, c);
          const m = measureUi(id, mode, c);
          const reading = m ? `WCAG ${m.ratio.toFixed(2)}` : "a colour is missing";
          const run = KNOWN_FAILURES.includes(label) ? it.fails : it;
          run(`${label} >= ${c.min} (${reading})`, () => {
            expect(m, `${label}: a colour is missing`).toBeDefined();
            expect((m?.ratio ?? 0) + 1e-9, label).toBeGreaterThanOrEqual(c.min);
          });
        }
      });
    }
  }
}
