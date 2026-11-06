// @vitest-environment jsdom

import { act, cleanup, render, screen } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";
import useAskDock from "@/stores/useAskDock";
import { AskDock } from ".";

/**
 * ⌘K must land the caret in the composer, every time (#3970). The composer's
 * own `autoFocus` covers only the first open: the dock stays mounted across
 * collapses, so a reopen used to leave focus on the page and swallow the first
 * keystrokes. The composer here keeps `autoFocus` like the real one, so the
 * first open passes either way and the REOPEN is the case that matters.
 */

vi.mock("./DockComposer", () => ({
  // biome-ignore lint/a11y/noAutofocus: mirrors the real ChatPanel textarea
  DockComposer: () => <textarea data-testid='composer' autoFocus />
}));
vi.mock("./DockHeader", () => ({ DockHeader: () => null }));
vi.mock("./DockHistory", () => ({ DockHistory: () => null }));
vi.mock("./DockThread", () => ({ DockThread: () => null }));
vi.mock("./useDockWidth", () => ({
  useDockWidth: () => ({
    width: 400,
    isDesktop: true,
    dragging: false,
    minWidth: 320,
    maxWidth: 800,
    handleProps: {}
  })
}));

afterEach(() => {
  cleanup();
  act(() => {
    useAskDock.setState({ isOpen: false, view: "composer", threadId: null });
  });
});

describe("AskDock focus", () => {
  it("focuses the composer on every open, not just the first", () => {
    render(
      <>
        <button type='button' data-testid='page'>
          page
        </button>
        <AskDock />
      </>
    );

    act(() => {
      useAskDock.getState().toggle();
    });
    expect(document.activeElement).toBe(screen.getByTestId("composer"));

    screen.getByTestId("page").focus();
    act(() => {
      useAskDock.getState().toggle();
    });
    act(() => {
      useAskDock.getState().toggle();
    });
    expect(document.activeElement).toBe(screen.getByTestId("composer"));
  });

  it("leaves focus alone while the dock is closed", () => {
    render(
      <>
        <button type='button' data-testid='page'>
          page
        </button>
        <AskDock />
      </>
    );
    act(() => {
      useAskDock.getState().toggle();
    });
    screen.getByTestId("page").focus();
    act(() => {
      useAskDock.getState().close();
    });
    expect(document.activeElement).toBe(screen.getByTestId("page"));
  });
});
