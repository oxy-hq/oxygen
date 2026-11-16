// @vitest-environment jsdom
//
// The lift is four writes in a row with no transaction behind it, so the two
// things worth pinning are: who ends up reading the secret, and what the user
// is told when it stops halfway.
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { toast } from "sonner";
import { afterEach, describe, expect, it, vi } from "vitest";
import { SecretService } from "@/services/secretService";
import type { AppSecretNeeds } from "@/types/sharedSecrets";

vi.mock("@/hooks/useCurrentProjectBranch", () => ({
  default: () => ({ project: { id: "proj-1" }, branchName: "main" })
}));
vi.mock("@/services/secretService", () => ({
  SecretService: {
    revealSecret: vi.fn(),
    createSecret: vi.fn(),
    shareWithApps: vi.fn(),
    deleteSecret: vi.fn()
  }
}));
vi.mock("sonner", () => ({ toast: { success: vi.fn(), error: vi.fn() } }));

import { LiftAppSecretDialog } from "./LiftAppSecretDialog";

const revealSecret = vi.mocked(SecretService.revealSecret);
const createSecret = vi.mocked(SecretService.createSecret);
const shareWithApps = vi.mocked(SecretService.shareWithApps);
const deleteSecret = vi.mocked(SecretService.deleteSecret);

const app = (id: string, name: string): AppSecretNeeds => ({
  app_id: id,
  app_name: name,
  app_slug: name.toLowerCase().replace(/ /g, "-"),
  missing: []
});

// Three, so that Select all / Clear renders: it only appears when more than
// one app is actually toggleable, and Platform Pulse is locked.
const apps = [app("pulse", "Platform Pulse"), app("ops", "Ops Bot"), app("rep", "Reports")];

/** Platform Pulse owns `STRIPE_KEY` as `apps/pulse/STRIPE_KEY`. */
const target = {
  secretId: "apps/pulse/STRIPE_KEY",
  key: "STRIPE_KEY",
  appId: "pulse",
  appName: "Platform Pulse"
};

const open = () => {
  const onDone = vi.fn();
  render(
    <QueryClientProvider
      client={new QueryClient({ defaultOptions: { mutations: { retry: false } } })}
    >
      <LiftAppSecretDialog
        target={target}
        apps={apps}
        nameTaken={false}
        onOpenChange={vi.fn()}
        onDone={onDone}
      />
    </QueryClientProvider>
  );
  return onDone;
};

const happy = () => {
  revealSecret.mockResolvedValue("sk_live_xxx");
  createSecret.mockResolvedValue({ id: "ws-stripe" } as never);
  shareWithApps.mockResolvedValue({} as never);
  deleteSecret.mockResolvedValue(undefined as never);
};

const confirm = () => fireEvent.click(screen.getByTestId("lift-app-secret-confirm"));

afterEach(() => {
  cleanup();
  vi.clearAllMocks();
});

describe("LiftAppSecretDialog", () => {
  it("keeps the owning app on the share, and will not let it be unticked", () => {
    happy();
    open();
    const owner = screen.getByLabelText("Platform Pulse");
    expect(owner).toBeChecked();
    expect(owner).toBeDisabled();
    // Clicking it changes nothing: its own copy is about to be deleted, so
    // dropping it from the share would leave it reading nothing.
    fireEvent.click(owner);
    expect(owner).toBeChecked();
  });

  it("shares with the owning app even when Clear leaves nothing ticked", async () => {
    happy();
    open();
    // Select all, then Clear: both act on the apps they can act on, never on
    // the locked owner.
    fireEvent.click(screen.getByTestId("secret-app-checklist-toggle-all"));
    expect(screen.getByLabelText("Ops Bot")).toBeChecked();
    fireEvent.click(screen.getByTestId("secret-app-checklist-toggle-all"));
    expect(screen.getByLabelText("Ops Bot")).not.toBeChecked();
    expect(screen.getByLabelText("Reports")).not.toBeChecked();
    expect(screen.getByLabelText("Platform Pulse")).toBeChecked();
    confirm();
    await waitFor(() => expect(shareWithApps).toHaveBeenCalled());
    expect(shareWithApps).toHaveBeenCalledWith("proj-1", "ws-stripe", ["pulse"]);
    // Only then is the app's own copy removed.
    expect(deleteSecret).toHaveBeenCalledWith("proj-1", "apps/pulse/STRIPE_KEY");
  });

  it("shares with the apps chosen and the owner, without duplicating it", async () => {
    happy();
    open();
    fireEvent.click(screen.getByLabelText("Ops Bot"));
    confirm();
    await waitFor(() => expect(shareWithApps).toHaveBeenCalled());
    const [, , appIds] = shareWithApps.mock.calls[0];
    expect([...(appIds as string[])].sort()).toEqual(["ops", "pulse"]);
  });

  it("takes the workspace copy back when the share fails, and says so", async () => {
    happy();
    shareWithApps.mockRejectedValue(new Error("nope"));
    const onDone = open();
    confirm();
    await waitFor(() => expect(toast.error).toHaveBeenCalled());
    // The created secret is removed, not left unshared on the other tab.
    expect(deleteSecret).toHaveBeenCalledWith("proj-1", "ws-stripe");
    expect(deleteSecret).not.toHaveBeenCalledWith("proj-1", "apps/pulse/STRIPE_KEY");
    const [, options] = vi.mocked(toast.error).mock.calls[0];
    expect(options?.description).toMatch(/removed\. Nothing changed/i);
    // The server's own sentence rides along, after the state, not instead.
    expect(options?.description).toContain("(nope)");
    expect(onDone).not.toHaveBeenCalled();
  });

  it("names what is left behind when the app's own copy cannot be deleted", async () => {
    happy();
    deleteSecret.mockRejectedValue(new Error("nope"));
    open();
    confirm();
    await waitFor(() => expect(toast.error).toHaveBeenCalled());
    const [, options] = vi.mocked(toast.error).mock.calls[0];
    // The move has not taken effect: the app's own value still wins.
    expect(options?.description).toContain("own value wins");
    expect(options?.description).toContain("Platform Pulse");
    // And exactly one toast: the delete hook is silenced so it cannot report
    // a success beside this.
    expect(vi.mocked(toast.error).mock.calls).toHaveLength(1);
    expect(toast.success).not.toHaveBeenCalled();
  });

  it("says nothing changed when the reveal fails, because nothing did", async () => {
    revealSecret.mockRejectedValue(new Error("nope"));
    open();
    confirm();
    await waitFor(() => expect(toast.error).toHaveBeenCalled());
    expect(createSecret).not.toHaveBeenCalled();
    const [, options] = vi.mocked(toast.error).mock.calls[0];
    expect(options?.description).toMatch(/Nothing was changed/i);
    expect(options?.description).toContain("Platform Pulse");
  });
});
