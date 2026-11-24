// @vitest-environment jsdom
import { cleanup, fireEvent, render, screen } from "@testing-library/react";
import { MemoryRouter } from "react-router-dom";
import { afterEach, describe, expect, it, vi } from "vitest";
import type { VisibleNavGroup } from "../nav";
import { SectionActions, SectionChrome } from "./SectionChrome";
import { SettingsNav } from "./SettingsNav";

/**
 * The rail and the section header carry the accessibility contract a design critique found
 * missing, so each clause of it is asserted here rather than left to a screenshot.
 */

const GROUPS: VisibleNavGroup[] = [
  {
    label: "You",
    items: [
      {
        value: "account.notifications",
        label: "Notifications",
        icon: () => null,
        description: "How and when Oxygen tells you about things.",
        keywords: ["quiet hours"]
      },
      { value: "preferences.appearance", label: "Appearance", icon: () => null, description: "d" }
    ]
  },
  {
    label: "Organization",
    subtitle: "Acme",
    items: [{ value: "organization.general", label: "General", icon: () => null, description: "d" }]
  }
];

const renderNav = (props: Partial<React.ComponentProps<typeof SettingsNav>> = {}) => {
  const onQueryChange = vi.fn();
  // The rows are real links now, so they need a router: a settings row is something you can
  // middle-click, copy, or open in a new tab, which a button never was.
  const view = render(
    <MemoryRouter initialEntries={["/acme/settings/preferences/appearance"]}>
      <SettingsNav
        groups={GROUPS}
        activeSection='preferences.appearance'
        hrefFor={(target) => `/acme/settings/${target.split(".").join("/")}`}
        query=''
        onQueryChange={onQueryChange}
        {...props}
      />
    </MemoryRouter>
  );
  return { ...view, onQueryChange };
};

afterEach(cleanup);

describe("SettingsNav", () => {
  it("names its landmark, so it can be told from the dialog's other regions", () => {
    renderNav();
    expect(screen.getByRole("navigation", { name: "Settings sections" })).toBeTruthy();
  });

  it("marks the active item with aria-current, not a data attribute alone", () => {
    // The active row used to be signalled only by `data-active` and a background fill that
    // was the same token as hover — nothing a screen reader or a careful eye could use.
    renderNav();
    const active = screen.getByRole("link", { name: "Appearance" });
    expect(active.getAttribute("aria-current")).toBe("page");
    expect(screen.getByRole("link", { name: "General" }).getAttribute("aria-current")).toBeNull();
  });

  it("ties each list to its group heading, so two items with one label stay distinct", () => {
    renderNav();
    for (const label of ["You", "Organization"]) {
      const heading = screen.getByRole("heading", { name: new RegExp(`^${label}`) });
      const list = screen.getByRole("list", { name: new RegExp(`^${label}`) });
      expect(list.getAttribute("aria-labelledby")).toBe(heading.id);
    }
  });

  it("keeps one tab stop, and moves between rows with the arrow keys", () => {
    // 22 sections meant up to 22 tab stops before the content. The active row is the only
    // stop; arrows do the rest.
    renderNav();
    const items = screen.getAllByRole("link").filter((b) => b.hasAttribute("data-nav-item"));
    expect(items.filter((b) => b.tabIndex === 0).map((b) => b.textContent)).toEqual(["Appearance"]);

    const first = screen.getByRole("link", { name: "Notifications" });
    first.focus();
    fireEvent.keyDown(first, { key: "ArrowDown" });
    expect(document.activeElement).toBe(screen.getByRole("link", { name: "Appearance" }));
    fireEvent.keyDown(document.activeElement as HTMLElement, { key: "End" });
    expect(document.activeElement).toBe(screen.getByRole("link", { name: "General" }));
    // Wraps, rather than dead-ending at the last row.
    fireEvent.keyDown(document.activeElement as HTMLElement, { key: "ArrowDown" });
    expect(document.activeElement).toBe(screen.getByRole("link", { name: "Notifications" }));
  });

  it("keeps a tab stop among the matches when the filter hides the active section", () => {
    // Review finding F2. The tab stop was keyed to the active section, so filtering it out
    // left every rendered row at tabIndex -1: Tab skipped the result list entirely, and the
    // arrow keys live on the rows, so the filter's own results became unreachable.
    renderNav({ query: "general" });
    const items = screen.getAllByRole("link").filter((b) => b.hasAttribute("data-nav-item"));
    expect(items.map((b) => b.textContent)).toEqual(["General"]);
    expect(items.filter((b) => b.tabIndex === 0).map((b) => b.textContent)).toEqual(["General"]);
    // ...and it is not the active one, which the filter removed.
    expect(screen.queryByRole("link", { name: "Appearance" })).toBeNull();
  });

  it("walks from the filter field into the results with ArrowDown", () => {
    renderNav({ query: "general" });
    const field = screen.getByTestId("settings-nav-filter");
    field.focus();
    fireEvent.keyDown(field, { key: "ArrowDown" });
    expect(document.activeElement).toBe(screen.getByRole("link", { name: "General" }));
  });

  it("gives every row a real address, so it can be opened in a new tab", () => {
    // A settings page has a URL now. These were buttons calling a store setter, so
    // middle-click, copy-link and open-in-new-tab all did nothing.
    renderNav();
    expect(screen.getByRole("link", { name: "General" }).getAttribute("href")).toBe(
      "/acme/settings/organization/general"
    );
    expect(screen.getByRole("link", { name: "Notifications" }).getAttribute("href")).toBe(
      "/acme/settings/account/notifications"
    );
  });

  it("filters to what matches, and says how many out loud", () => {
    renderNav({ query: "quiet hours" });
    expect(screen.getByRole("link", { name: "Notifications" })).toBeTruthy();
    expect(screen.queryByRole("link", { name: "General" })).toBeNull();
    // Announced as well as drawn, so the result is not silence for a screen reader.
    expect(screen.getByText("1 setting matches")).toBeTruthy();
  });

  it("says so when nothing matches, rather than showing an empty rail", () => {
    renderNav({ query: "zzzz" });
    expect(screen.getByTestId("settings-nav-empty").textContent).toContain("zzzz");
    expect(screen.getByText("0 settings match")).toBeTruthy();
  });

  it("offers a way to clear the filter", () => {
    const { onQueryChange } = renderNav({ query: "zzzz" });
    fireEvent.click(screen.getByRole("button", { name: "Clear filter" }));
    expect(onQueryChange).toHaveBeenCalledWith("");
  });
});

describe("SectionChrome", () => {
  it("heads the section with an h2 and its description", () => {
    // An h2 under the dialog's h1. The old header was an h3 with no h2 anywhere above it.
    render(
      <SectionChrome title='Notifications' description='How and when Oxygen tells you.'>
        <p>body</p>
      </SectionChrome>
    );
    const heading = screen.getByRole("heading", { level: 2, name: "Notifications" });
    expect(heading).toBeTruthy();
    expect(screen.getByText("How and when Oxygen tells you.")).toBeTruthy();
  });

  it("is focusable programmatically without joining the tab order", () => {
    // The shell focuses it on a section change so the move is announced; it must not add a
    // tab stop of its own.
    render(<SectionChrome title='Members'>{null}</SectionChrome>);
    const heading = screen.getByRole("heading", { level: 2, name: "Members" });
    expect(heading.tabIndex).toBe(-1);
    heading.focus();
    expect(document.activeElement).toBe(heading);
  });

  it("lifts a section's actions up beside the title", () => {
    render(
      <SectionChrome title='Teams'>
        <div data-testid='body'>
          <SectionActions>
            <button type='button'>New team</button>
          </SectionActions>
        </div>
      </SectionChrome>
    );
    const button = screen.getByRole("button", { name: "New team" });
    // It renders in the header, not where it was written.
    expect(screen.getByTestId("body").contains(button)).toBe(false);
    expect(
      screen.getByRole("heading", { level: 2 }).parentElement?.parentElement?.contains(button)
    ).toBe(true);
  });

  it("renders a section's actions in place when there is no header above them", () => {
    // A section mounted on its own — which is how its own tests mount it — still shows its
    // buttons, rather than losing them to a portal target that does not exist.
    render(
      <div data-testid='standalone'>
        <SectionActions>
          <button type='button'>Create</button>
        </SectionActions>
      </div>
    );
    const button = screen.getByRole("button", { name: "Create" });
    expect(screen.getByTestId("standalone").contains(button)).toBe(true);
  });
});
