// @vitest-environment jsdom

import { cleanup, render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, beforeAll, describe, expect, it, vi } from "vitest";
import type { AppAccessSummary } from "@/types/appAccess";
import { NewKioskDialog } from "./NewKioskDialog";

// Mounting a shadcn Dialog under jsdom is slow enough that the 5s default
// trips once a Select is opened inside it.
vi.setConfig({ testTimeout: 20000 });

// Radix's Select reaches for pointer-capture and scrollIntoView, neither of
// which jsdom implements. Without them the trigger click throws instead of
// opening the listbox.
beforeAll(() => {
  Element.prototype.hasPointerCapture ??= () => false;
  Element.prototype.setPointerCapture ??= () => {};
  Element.prototype.releasePointerCapture ??= () => {};
  Element.prototype.scrollIntoView ??= () => {};
});

const createDevice = vi.fn();

// The hooks are the seam. What this file is about is where a new tablet lands
// — the `return_to` the create call carries, or its absence — not React
// Query's plumbing.
vi.mock("@/hooks/api/organizations", () => ({
  useCreateDevice: () => ({ mutateAsync: createDevice, isPending: false }),
  useLocations: () => ({ data: [] })
}));

afterEach(() => {
  cleanup();
  createDevice.mockReset();
});

const ORG_SLUG = "pokehouse";

const app = (id: string, name: string, over: Partial<AppAccessSummary> = {}): AppAccessSummary => ({
  id,
  name,
  slug: name.toLowerCase().replace(/\s+/g, "-"),
  visibility: "org",
  grant_count: 0,
  published: true,
  ...over
});

const STORE_OPS = app("a1", "Store Ops");
const INVENTORY = app("a2", "Inventory");
// A draft's shell refuses to load, so a kiosk can't open it: it is offered
// (an admin may be about to publish it) but never guessed.
const DRAFT = app("a3", "Scheduling", { published: false });

const dialog = (apps: AppAccessSummary[]) => {
  const user = userEvent.setup({ delay: null });
  const view = render(
    <NewKioskDialog
      open
      onOpenChange={() => {}}
      orgId='org-1'
      orgSlug={ORG_SLUG}
      apps={apps}
      onCreated={() => {}}
    />
  );
  return { user, view };
};

const opens = () => screen.getByTestId("settings-crew-kiosk-opens");
const submit = () => screen.getByTestId("settings-crew-new-kiosk-submit") as HTMLButtonElement;
const nameIt = (user: ReturnType<typeof userEvent.setup>) =>
  user.type(screen.getByLabelText("Name"), "Front counter");
const sentRequest = () => createDevice.mock.calls[0][0].request;
const appUrl = (slug: string) => `${window.location.origin}/customer-apps/${ORG_SLUG}/${slug}/`;

describe("NewKioskDialog — where the tablet opens", () => {
  it("preselects the org's one app a kiosk can open, and sends it", async () => {
    createDevice.mockResolvedValue({});
    const { user } = dialog([STORE_OPS, DRAFT]);
    expect(opens().textContent).toBe("Store Ops");
    await nameIt(user);
    await user.click(submit());
    await waitFor(() => expect(createDevice).toHaveBeenCalledTimes(1));
    expect(sentRequest()).toEqual({ name: "Front counter", return_to: appUrl("store-ops") });
  });

  it("preselects the one app when the list arrives after the dialog opened", async () => {
    // The app list is its own query; a dialog opened before it answers must
    // not have frozen "Organization home" in as the choice.
    const { view } = dialog([]);
    expect(opens().textContent).toBe("Organization home");
    view.rerender(
      <NewKioskDialog
        open
        onOpenChange={() => {}}
        orgId='org-1'
        orgSlug={ORG_SLUG}
        apps={[STORE_OPS]}
        onCreated={() => {}}
      />
    );
    expect(opens().textContent).toBe("Store Ops");
  });

  it("does not guess between several apps: Create waits for a choice, with the reason", async () => {
    createDevice.mockResolvedValue({});
    const { user } = dialog([STORE_OPS, INVENTORY]);
    expect(opens().textContent).toBe("Choose where the tablet lands");
    await nameIt(user);
    expect(submit().disabled).toBe(true);
    expect(screen.getByTestId("settings-crew-new-kiosk-blocked").textContent).toContain(
      "Choose where the tablet lands"
    );

    await user.click(opens());
    await user.click(await screen.findByRole("option", { name: "Inventory" }));
    expect(submit().disabled).toBe(false);
    expect(screen.queryByTestId("settings-crew-new-kiosk-blocked")).toBeNull();
    await user.click(submit());
    await waitFor(() => expect(createDevice).toHaveBeenCalledTimes(1));
    expect(sentRequest()).toEqual({ name: "Front counter", return_to: appUrl("inventory") });
  });

  it("still takes Organization home among several apps, as a deliberate choice", async () => {
    createDevice.mockResolvedValue({});
    const { user } = dialog([STORE_OPS, INVENTORY]);
    await nameIt(user);
    await user.click(opens());
    await user.click(await screen.findByRole("option", { name: "Organization home" }));
    expect(submit().disabled).toBe(false);
    await user.click(submit());
    await waitFor(() => expect(createDevice).toHaveBeenCalledTimes(1));
    // No `return_to` at all: the column stays NULL, which is what "home" means.
    expect(sentRequest()).toEqual({ name: "Front counter" });
  });

  it("keeps Organization home when the org has no app a kiosk can open", async () => {
    createDevice.mockResolvedValue({});
    const { user } = dialog([DRAFT]);
    expect(opens().textContent).toBe("Organization home");
    expect(screen.getByTestId("settings-crew-kiosk-opens-hint").textContent).toContain(
      "No app is published"
    );
    await nameIt(user);
    expect(submit().disabled).toBe(false);
    await user.click(submit());
    await waitFor(() => expect(createDevice).toHaveBeenCalledTimes(1));
    expect(sentRequest()).toEqual({ name: "Front counter" });
  });

  it("says under Opens what the choice does for crew", () => {
    dialog([STORE_OPS]);
    expect(screen.getByTestId("settings-crew-kiosk-opens-hint").textContent).toContain(
      "Crew land in Store Ops"
    );
  });
});
