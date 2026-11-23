// @vitest-environment jsdom

import { act, cleanup, render, screen } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import useAskDock from "@/stores/useAskDock";
import useNotificationsDock from "@/stores/useNotificationsDock";
import { NotificationsDock } from "./NotificationsDock";

/**
 * The notifications panel shares the right edge with Ask: one of the two
 * at a time. It stays mounted while shut, inert and at no width, and reads the
 * inbox only while open; under 768px the bell's sheet takes over and it is gone.
 */

let wide = true;
vi.mock("@/hooks/useMediaQuery", () => ({ useMediaQuery: () => wide }));
const listOpen = vi.fn();
vi.mock("@/components/Notifications/NoticeList", () => ({
  NoticeList: ({ open }: { open: boolean }) => {
    listOpen(open);
    return <div data-testid='notice-list' />;
  }
}));

beforeEach(() => {
  useAskDock.setState({ isOpen: false });
  useNotificationsDock.setState({ isOpen: false });
});
afterEach(() => {
  cleanup();
  wide = true;
  vi.clearAllMocks();
});

describe("the notifications dock", () => {
  it("is shut, inert and reads nothing until the bell opens it", () => {
    render(<NotificationsDock />);
    const dock = screen.getByTestId("notifications-dock");
    expect(dock.style.width).toBe("0px");
    expect(dock.hasAttribute("inert")).toBe(true);
    expect(listOpen).toHaveBeenLastCalledWith(false);
    act(() => {
      useNotificationsDock.getState().toggle();
    });
    expect(dock.style.width).toBe("440px");
    expect(dock.hasAttribute("inert")).toBe(false);
    expect(listOpen).toHaveBeenLastCalledWith(true);
  });

  it("closes Ask when it opens, and closes when Ask opens", () => {
    render(<NotificationsDock />);
    act(() => {
      useAskDock.getState().open();
    });
    act(() => {
      useNotificationsDock.getState().open();
    });
    expect(useAskDock.getState().isOpen).toBe(false);
    act(() => {
      useAskDock.getState().toggle();
    });
    expect(useNotificationsDock.getState().isOpen).toBe(false);
    expect(screen.getByTestId("notifications-dock").style.width).toBe("0px");
  });

  it("is not there under 768px, where the bell opens a sheet", () => {
    wide = false;
    render(<NotificationsDock />);
    expect(screen.queryByTestId("notifications-dock")).toBeNull();
  });
});
