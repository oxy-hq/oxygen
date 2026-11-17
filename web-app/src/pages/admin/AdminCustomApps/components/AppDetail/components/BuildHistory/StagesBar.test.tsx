// @vitest-environment jsdom

import { cleanup, render, screen } from "@testing-library/react";
import { afterEach, describe, expect, it } from "vitest";
import { ADMIN_TONE } from "@/pages/admin/components/adminTone";
import { StagesBar } from "./StagesBar";

afterEach(cleanup);

const STAGES = [
  { stage: "admit", ms: 12 },
  { stage: "inflate", ms: 340 },
  { stage: "store", ms: 0 },
  { stage: "record", ms: 1648 }
];

const growOf = (el: HTMLElement) => Number(el.style.flexGrow);

describe("StagesBar", () => {
  it("draws a segment per stage that took time, sized by its share, in order", () => {
    render(<StagesBar stages={STAGES} testId='bar' />);
    const bar = screen.getByRole("img");
    const segments = [...bar.children] as HTMLElement[];
    expect(segments.map((s) => s.dataset.testid)).toEqual([
      "bar-segment-admit",
      "bar-segment-inflate",
      "bar-segment-record"
    ]);
    expect(segments.map(growOf)).toEqual([0.006, 0.17, 0.824]);
  });

  it("labels every stage with its time, the one that took none included, and the total", () => {
    render(<StagesBar stages={STAGES} testId='bar' />);
    expect(screen.getByTestId("bar-label-admit").textContent).toBe("admit 12ms");
    expect(screen.getByTestId("bar-label-store").textContent).toBe("store 0ms");
    expect(screen.getByTestId("bar-label-record").textContent).toBe("record 1.6s");
    expect(screen.getByTestId("bar-total").textContent).toBe("total 2.0s");
    expect(screen.getByRole("img").getAttribute("aria-label")).toBe(
      "Stages: admit 12ms, inflate 340ms, store 0ms, record 1.6s; total 2.0s"
    );
  });

  it("colours nothing on a publish that landed", () => {
    const { container } = render(<StagesBar stages={STAGES} testId='bar' />);
    expect(container.innerHTML).not.toContain(ADMIN_TONE.danger.dot);
    expect(container.innerHTML).not.toContain(ADMIN_TONE.danger.text);
  });

  it("colours the stage a refused publish failed at, and only it", () => {
    render(<StagesBar stages={STAGES.slice(0, 2)} failedStage='inflate' testId='bar' />);
    expect(screen.getByTestId("bar-segment-inflate").className).toContain(ADMIN_TONE.danger.dot);
    expect(screen.getByTestId("bar-segment-admit").className).not.toContain(ADMIN_TONE.danger.dot);
    expect(screen.getByTestId("bar-label-inflate").className).toContain(ADMIN_TONE.danger.text);
    expect(screen.getByTestId("bar-label-admit").className).not.toContain(ADMIN_TONE.danger.text);
  });

  it("renders nothing for a publish with no timings", () => {
    const { container } = render(<StagesBar stages={[]} testId='bar' />);
    expect(container.innerHTML).toBe("");
  });
});
