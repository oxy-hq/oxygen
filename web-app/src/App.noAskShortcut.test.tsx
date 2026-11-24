// @vitest-environment jsdom

import { act, cleanup, render, screen } from "@testing-library/react";
import { MemoryRouter } from "react-router-dom";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import useAskDock from "@/stores/useAskDock";
import { WorkspaceLayout } from "./App";

/**
 * ⌘K / Ctrl+K opens nothing on a workspace page. It used to toggle the Ask dock
 * (a window listener in `WorkspaceLayout`); the product dropped that last way in,
 * so the key is left to the browser: the dock's store stays closed and the event
 * is not prevented. The admin console's own ⌘K (`AdminEntitySearch`) is a
 * different page and is not covered here.
 *
 * Renders the real `WorkspaceLayout` for one workspace; everything it mounts that
 * fetches (the shell, the settings dialog, the home) is a stub.
 */

const workspace = { id: "ws-1", name: "Demo" };

// An animation library that wants a canvas jsdom does not have; not under test.
vi.mock("@lottiefiles/react-lottie-player", () => ({ Player: () => null }));
// Vite's `define`s (vite.config.ts), which vitest does not apply; App.tsx's
// imports read them at load.
vi.hoisted(() => {
  Object.assign(globalThis, { __APP_VERSION__: "test", __OXY_OAUTH_PROXY_ORIGIN__: "" });
});

vi.mock("./contexts/AuthContext", async (importOriginal) => ({
  ...(await importOriginal<object>()),
  useAuth: () => ({ authConfig: {}, isLocalMode: false })
}));
vi.mock("./hooks/api/workspaces/useWorkspaces", async (importOriginal) => ({
  ...(await importOriginal<object>()),
  useWorkspace: () => ({
    isPending: false,
    isError: false,
    error: null,
    data: workspace,
    isMaterializing: false,
    materializingTimedOut: false,
    refetch: vi.fn()
  })
}));
vi.mock("./components/Shell/WorkspaceShell", () => ({
  WorkspaceShell: ({ children }: { children: React.ReactNode }) => (
    <div data-testid='shell'>{children}</div>
  )
}));
vi.mock("./contexts/PreviewPinContext", async (importOriginal) => ({
  ...(await importOriginal<object>()),
  PreviewPinProvider: ({ children }: { children: React.ReactNode }) => children
}));
vi.mock("./components/settings/SettingsDialog", () => ({ default: () => null }));
vi.mock("./components/settings/SettingsDialog/useSettingsDeepLink", async (importOriginal) => ({
  ...(await importOriginal<object>()),
  useSettingsDeepLink: () => {}
}));
vi.mock("./hooks/useOauthConnectReturn", () => ({ default: () => {} }));
vi.mock("@/pages/launcher/CustomerHome", () => ({
  default: () => <div data-testid='customer-home' />
}));

function press(init: KeyboardEventInit): KeyboardEvent {
  const event = new KeyboardEvent("keydown", {
    key: "k",
    bubbles: true,
    cancelable: true,
    ...init
  });
  act(() => {
    window.dispatchEvent(event);
  });
  return event;
}

beforeEach(() => {
  useAskDock.setState({ isOpen: false });
});
afterEach(cleanup);

describe("WorkspaceLayout keyboard", () => {
  it.each([
    ["⌘K", { metaKey: true }],
    ["Ctrl+K", { ctrlKey: true }]
  ])("%s on the home leaves the Ask dock closed and the key to the browser", async (_, mods) => {
    render(
      <MemoryRouter initialEntries={["/"]}>
        <WorkspaceLayout workspaceId={workspace.id} />
      </MemoryRouter>
    );
    await screen.findByTestId("customer-home");

    const event = press(mods);

    expect(useAskDock.getState().isOpen).toBe(false);
    expect(event.defaultPrevented).toBe(false);
  });
});
