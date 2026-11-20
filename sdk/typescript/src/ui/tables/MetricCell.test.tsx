// @vitest-environment jsdom
//
// One measure in a cell: the step a value reaches against the app's thresholds (either way up),
// and the look each step wears: the status tint only for warning and error, weight for notable,
// the quiet ink for clear, an asterisk for provisional, words alone for nothing to judge. The
// comparison shows only where the cell asserts something, and every state carries its sentence.

import { afterAll, afterEach, describe, expect, it } from "vitest";
import { type PaintMode, paintUi, rgb, themeValue, unpaintUi } from "../paint.harness";
import { ui } from "../recipes";
import { $, fire, render } from "../ui.harness";
import { MetricCell, type MetricThresholds, metricStep } from "./MetricCell";

afterEach(() => {
  document.body.innerHTML = "";
});

const COST: MetricThresholds = { notable: 1, warning: 1.5, error: 4 };
const RATING: MetricThresholds = { warning: 4.3, error: 4.0, direction: "below" };

const cell = () => $(".oxy-ui-metric-cell") as HTMLElement;
const figure = () => $(".oxy-ui-metric-cell__figure") as HTMLElement;

describe("metricStep", () => {
  it("climbs at each threshold, a figure at the bar having reached it", () => {
    expect(metricStep(0.5, COST)).toBe("clear");
    expect(metricStep(1, COST)).toBe("notable");
    expect(metricStep(1.49, COST)).toBe("notable");
    expect(metricStep(1.5, COST)).toBe("warning");
    expect(metricStep(4, COST)).toBe("error");
    expect(metricStep(40, COST)).toBe("error");
  });

  it("reads the other way for a measure where smaller is worse", () => {
    expect(metricStep(4.6, RATING)).toBe("clear");
    expect(metricStep(4.3, RATING)).toBe("warning");
    expect(metricStep(4.0, RATING)).toBe("error");
    expect(metricStep(3.2, RATING)).toBe("error");
  });

  it("has no step without a value, and none it was not given", () => {
    expect(metricStep(null, COST)).toBeNull();
    expect(metricStep(undefined, COST)).toBeNull();
    expect(metricStep(Number.NaN, COST)).toBeNull();
    expect(metricStep(2, { warning: 1.5 })).toBe("warning");
    expect(metricStep(9, { warning: 1.5 })).toBe("warning");
  });
});

describe("MetricCell tones", () => {
  it("tints a warning figure with the warning tone and shows its comparison", async () => {
    await render(<MetricCell observed='24.0%' comparison='17.4%' value={2} thresholds={COST} />);
    expect(cell().dataset.step).toBe("warning");
    expect(figure().className).toContain(ui.chipTone.warning);
    expect(figure().className).toContain("oxy-ui-metric-cell__figure--tinted");
    expect(cell().textContent).toContain("24.0%vs 17.4%");
  });

  it("tints an error figure with the error tone", async () => {
    await render(<MetricCell observed='51%' comparison='12%' value={4.2} thresholds={COST} />);
    expect(cell().dataset.step).toBe("error");
    expect(figure().className).toContain(ui.chipTone.error);
    expect(figure().className).not.toContain(ui.chipTone.warning);
  });

  it("marks a notable figure by weight alone, still with its comparison", async () => {
    await render(<MetricCell observed='19%' comparison='17%' value={1.1} thresholds={COST} />);
    expect(cell().dataset.step).toBe("notable");
    expect(figure().className).toContain("oxy-ui-metric-cell__figure--notable");
    expect(figure().className).not.toMatch(/oxy-ui-tone-/);
    expect($(".oxy-ui-metric-cell__comparison")?.textContent).toBe("vs 17%");
  });

  it("keeps a clear figure quiet: no tint, no weight, the comparison left to its sentence", async () => {
    await render(
      <MetricCell
        observed='16%'
        comparison='17%'
        value={0.4}
        thresholds={COST}
        hint='Food cost 16% against 17%: in line.'
      />
    );
    expect(cell().dataset.step).toBe("clear");
    expect(figure().className).toBe("oxy-ui-metric-cell__figure");
    expect($(".oxy-ui-metric-cell__comparison")).toBeNull();
    expect($(".oxy-ui-metric-cell__hint")?.textContent).toBe("Food cost 16% against 17%: in line.");
  });

  it("says its sentence in a hint on hover, and to a screen reader as words", async () => {
    await render(
      <MetricCell observed='51%' value={5} thresholds={COST} hint='Food cost far above.' />
    );
    expect($("[role='tooltip']")).toBeNull();
    const frame = $(".oxy-ui-metric-cell__frame") as HTMLElement;
    expect(frame.contains(cell())).toBe(true);
    const over = new MouseEvent("pointerover", { bubbles: true });
    Object.defineProperty(over, "pointerType", { value: "mouse" });
    await fire(cell(), over);
    expect($("[role='tooltip']")?.textContent).toBe("Food cost far above.");
    // The words are in the cell too, hidden from the eye only.
    expect(cell().textContent).toContain("Food cost far above.");
  });

  it("draws no hint frame when it has no sentence", async () => {
    await render(<MetricCell observed='16%' step='clear' />);
    expect($(".oxy-ui-metric-cell__frame")).toBeNull();
    expect($(".oxy-ui-metric-cell__hint")).toBeNull();
  });

  it("takes a step the app judged, over its own thresholds", async () => {
    await render(<MetricCell observed='4.58' value={4.9} thresholds={RATING} step='error' />);
    expect(cell().dataset.step).toBe("error");
    expect(figure().className).toContain(ui.chipTone.error);
  });

  it("never tints a provisional figure: subtle ink, comparison and asterisk", async () => {
    await render(
      <MetricCell observed='29.8%' comparison='23.0%' value={9} thresholds={COST} provisional />
    );
    expect(cell().dataset.step).toBe("provisional");
    expect(figure().className).not.toMatch(/oxy-ui-tone-/);
    expect(figure().className).toContain("oxy-ui-metric-cell__figure--provisional");
    const asterisk = $(".oxy-ui-metric-cell__comparison span") as HTMLElement;
    expect(asterisk.textContent).toBe("*");
    expect(asterisk.title).toBe("Too few to compare against");
  });

  it("is words alone when there is nothing to judge, at the body size", async () => {
    await render(<MetricCell label='No books connected' hint='No books are connected yet.' />);
    expect(cell().dataset.step).toBe("empty");
    expect(figure()).toBeNull();
    const words = $(".oxy-ui-metric-cell__label") as HTMLElement;
    expect(words.textContent).toBe("No books connected");
    expect(words.className).not.toContain("--beside");
  });

  it("sets a label beside a figure as its caption, and says a differing period", async () => {
    await render(<MetricCell observed='16%' label='2nd highest of 9' asOf='Jul 2026' />);
    expect($(".oxy-ui-metric-cell__label")?.className).toContain(
      "oxy-ui-metric-cell__label--beside"
    );
    expect($(".oxy-ui-metric-cell__as-of")?.textContent).toBe("as of Jul 2026");
  });

  it("prints the comparison when asked, even on a clear figure", async () => {
    await render(<MetricCell observed='16%' comparison='17%' step='clear' showComparison />);
    expect($(".oxy-ui-metric-cell__comparison")?.textContent).toBe("vs 17%");
  });
});

// The ink each look prints in, with ui.css painted on (base.css's tone classes, then this group's
// rules, in build order): the classes are right and the cascade still has to let the tone win.
// A flagged figure prints its tone's ink on its tone's tint, exactly as the reference's
// `bg-status-warning-bg text-status-warning` (and `chipTone`) do; it lost that ink to the clear
// figure's dim rule once (SDK-GAPS 3: #444448 on #fbf0ce in the Stores shot, the reference's
// #865108).
describe("MetricCell ink, painted", () => {
  afterAll(() => unpaintUi());
  const ink = () => getComputedStyle(figure()).color;
  const fill = () => getComputedStyle(figure()).backgroundColor;

  for (const mode of ["light", "dark"] as PaintMode[]) {
    it(`prints a warning figure in the warning ink on the warning tint (${mode})`, async () => {
      paintUi({ mode });
      await render(<MetricCell observed='32.4%' comparison='24%' step='warning' />);
      expect(ink()).toBe(rgb(themeValue("--status-warning", mode)));
      expect(fill()).toBe(rgb(themeValue("--status-warning-bg", mode)));
      expect(getComputedStyle(figure()).fontWeight).toBe("600");
    });

    it(`prints an error figure in the error ink on the error tint (${mode})`, async () => {
      paintUi({ mode });
      await render(<MetricCell observed='51%' comparison='12%' step='error' />);
      expect(ink()).toBe(rgb(themeValue("--status-error", mode)));
      expect(fill()).toBe(rgb(themeValue("--status-error-bg", mode)));
    });

    it(`keeps the clear, notable and provisional inks (${mode})`, async () => {
      paintUi({ mode });
      const clear = await render(<MetricCell observed='16%' step='clear' />);
      expect(ink()).toBe(rgb(themeValue("--dim-foreground", mode)));
      await clear.unmount();
      const notable = await render(<MetricCell observed='19%' step='notable' />);
      expect(ink()).toBe(rgb(themeValue("--foreground", mode)));
      await notable.unmount();
      await render(<MetricCell observed='9%' comparison='8%' provisional />);
      expect(ink()).toBe(rgb(themeValue("--subtle-foreground", mode)));
    });
  }

  it("draws the reference's warning ink: #865108 light, #eab308 dark", () => {
    expect(themeValue("--status-warning", "light")).toBe("#865108");
    expect(themeValue("--status-warning", "dark")).toBe("#eab308");
  });
});
