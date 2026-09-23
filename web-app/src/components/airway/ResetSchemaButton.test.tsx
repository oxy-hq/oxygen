// @vitest-environment jsdom

import { cleanup, fireEvent, render, screen } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";

vi.mock("@/hooks/api/airway/useAirway", () => ({
  useResetSchema: () => ({ mutateAsync: vi.fn(), isPending: false })
}));
vi.mock("sonner", () => ({ toast: { success: vi.fn(), error: vi.fn() } }));

import ResetSchemaButton from "./ResetSchemaButton";

/**
 * This button drops every destination table. Until the cursor rewind existed it
 * was the only reset on the pipeline page, which meant it was also the button
 * reached for to do a rewind's job — once, on BMG's `amazon_vc`, one click away
 * from 3.7M rows of `vendor_forecasting` that Amazon only ever serves as of
 * today, in order to rewind one unrelated resource by 180 days.
 *
 * So the two things asserted here are safety properties, not styling:
 *
 * 1. **The cross-link exists.** To a reviewer who was not there it reads as copy
 *    polish. It is the sentence that intercepts the mistake at the exact moment
 *    it is being made, and it is the cheapest part of the whole change.
 * 2. **This button is not the heaviest thing on screen.** It is `ghost` and
 *    muted; the rewind beside it carries the `outline`. Flip those and the
 *    original failure is back.
 */

afterEach(cleanup);

const openDialog = () => {
  render(<ResetSchemaButton pipelineRef='pipelines/amazon_vc.airway.yml' />);
  fireEvent.click(screen.getByTestId("airway-reset-schema-button"));
};

describe("ResetSchemaButton — the cross-link to the non-destructive reset", () => {
  it("points at Rewind cursors for the case that does not need a drop", () => {
    openDialog();

    const hint = screen.getByTestId("airway-reset-schema-rewind-hint");
    expect(hint.textContent).toContain("Only need to re-pull from an earlier date?");
    expect(hint.textContent).toContain("Use Rewind cursors");
    // It has to say what the alternative preserves, or it is just a signpost to
    // another button whose consequence is equally unstated.
    expect(hint.textContent).toContain("keeps every row");
  });

  it("still states its own consequence in full", () => {
    openDialog();
    expect(screen.getByText("drops the destination tables")).toBeInTheDocument();
  });
});

describe("ResetSchemaButton — visible, but no longer the obvious one", () => {
  it("is de-emphasised rather than hidden", () => {
    render(<ResetSchemaButton pipelineRef='pipelines/amazon_vc.airway.yml' />);

    const button = screen.getByTestId("airway-reset-schema-button");
    // Findable: hiding a destructive action behind an overflow menu trades one
    // failure for another — someone who legitimately needs it goes hunting.
    expect(button).toBeVisible();
    // But muted, not destructive-coloured-and-outlined the way it used to be.
    expect(button.className).toContain("text-muted-foreground");
    // `cva` emits no `variant-*` class, so the check has to be on a class the
    // `outline` variant really carries. `border-input` is it — and `ghost`
    // does not emit it. (`text-muted-foreground` alone cannot catch the flip:
    // it is this button's own `className`, which survives any variant.)
    expect(button.className).not.toContain("border-input");
  });
});
