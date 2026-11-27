// @vitest-environment jsdom
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { cleanup, render, screen, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { AxiosError, type AxiosResponse } from "axios";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { AppBuildChecksService } from "@/services/api/appBuildChecks";
import type { BuildCheck, BuildChecks } from "@/types/appBuildChecks";
import { BuildChecksList } from "./components/BuildChecksList";

vi.mock("@/services/api/appBuildChecks", () => ({
  AppBuildChecksService: { get: vi.fn(), file: vi.fn(), markChangesSeen: vi.fn() }
}));
vi.mock("@/hooks/api/customApps/useAppFunctions", () => ({
  useInvocationCalls: () => ({ isPending: true, isError: false, data: undefined })
}));

const row = (over: Partial<BuildCheck>): BuildCheck => ({
  kind: "smoke",
  label: "Every screen opens",
  status: "passed",
  advisory: false,
  summary: null,
  detail: { findings: [] },
  notes: [],
  attempt: 2,
  run_id: "r",
  created_at: "2026-11-22T10:00:00Z",
  started_at: null,
  finished_at: null,
  duration_ms: null,
  ...over
});

const attempt = (checks: BuildCheck[], environment = "staging"): BuildChecks => ({
  app_id: "app-1",
  build: { id: "row-s", build_id: "9087d7b1c2d3e4f5", created_at: "2026-11-22T09:00:00Z" },
  attempt: 2,
  environment,
  state: "passed",
  counts: { total: 0, queued: 0, running: 0, passed: 0, failed: 0, error: 0, skipped: 0 },
  checks,
  summary: null,
  attempts: [{ attempt: 2, state: "passed", created_at: "2026-11-22T10:00:00Z" }],
  runner: null,
  stalled: null
});

const viewport = (name: string, over: Record<string, unknown> = {}) => ({
  name,
  width: name === "desk" ? 1440 : 390,
  height: 900,
  ready: true,
  ready_ms: 900,
  environment: "staging",
  build_id: "9087d7b",
  page_errors: [],
  failed_requests: [],
  refused_writes: [],
  ...over
});
const opened = (path: string, desk = {}, phone = {}) => ({
  path,
  viewports: [viewport("desk", desk), viewport("phone", phone)]
});

const shot = (path: string, view: string, over: Record<string, unknown> = {}) => ({
  path,
  viewport: view,
  changed: false,
  ratio: 0,
  staging: `shot-staging-${path || "root"}-${view}.png`,
  production: `shot-production-${path || "root"}-${view}.png`,
  diff: null,
  production_ready: true,
  production_errors: [],
  ...over
});
const CHANGES = {
  findings: [],
  production_build_id: "60e2f19aaaaaaaaa",
  compared: 4,
  changed: 2,
  screens: [
    shot("", "desk"),
    shot("stores", "desk", { changed: true, ratio: 0.031, diff: "diff-stores-desk.png" }),
    shot("", "phone"),
    shot("stores", "phone", { changed: true, ratio: 0.2, production: null })
  ],
  seen: null
};
const changes = (detail: object = CHANGES, over: Partial<BuildCheck> = {}) =>
  row({
    kind: "changes",
    label: "What changed",
    advisory: true,
    summary: "2 of 4 screens differ",
    detail,
    ...over
  });

const notFound = () =>
  new AxiosError("not found", "ERR", undefined, undefined, { status: 404 } as AxiosResponse);

const renderList = (checks: BuildCheck[], props: { seenRequired?: boolean; env?: string } = {}) =>
  render(
    <QueryClientProvider client={new QueryClient()}>
      <BuildChecksList
        appId='app-1'
        data={attempt(checks, props.env)}
        seenRequired={props.seenRequired}
      />
    </QueryClientProvider>
  );
const open = (kind: string) => userEvent.click(screen.getByTestId(`build-check-${kind}-toggle`));
const pictures = (within_: HTMLElement) =>
  within(within_)
    .getAllByTestId("build-check-picture")
    .map((p) => [p.dataset.file, p.dataset.state]);
const fileNames = () => vi.mocked(AppBuildChecksService.file).mock.calls.map(([, , name]) => name);
const isPrimary = (button: HTMLElement) => button.className.split(/\s+/).includes("bg-primary");

beforeEach(() => {
  let n = 0;
  URL.createObjectURL = vi.fn(() => `blob:picture-${++n}`);
  URL.revokeObjectURL = vi.fn();
  vi.mocked(AppBuildChecksService.file).mockResolvedValue(new Blob(["png"]));
});
afterEach(() => {
  cleanup();
  vi.clearAllMocks();
  vi.unstubAllGlobals();
});

describe("smoke evidence", () => {
  const SMOKE = row({
    status: "failed",
    summary: "1 of 3 screens have errors",
    detail: {
      findings: [],
      discovered: true,
      screens_omitted: 2,
      screens: [
        opened(""),
        opened(
          "stores",
          {
            failed_requests: [{ url: "https://acme/fn/list", status: 403, error: null }],
            refused_writes: ["POST /fn/save-plan"]
          },
          { ready: false, ready_ms: null, page_errors: ["TypeError: x is undefined"] }
        ),
        // Ready, and it threw: failing all the same.
        opened("flags", { page_errors: ["ReferenceError: y is not defined"] })
      ]
    }
  });

  it("draws one line per screen with both viewports, the failing screens first", () => {
    renderList([SMOKE]);
    const lines = screen.getAllByTestId("build-check-screen");
    expect(lines.map((li) => [li.dataset.path, li.dataset.failing])).toEqual([
      ["stores", "true"],
      ["flags", "true"],
      ["", "false"]
    ]);
    const flags = within(lines[1]);
    expect(flags.getByTestId("build-check-screen-desk").dataset.ok).toBe("false");
    expect(flags.getByTestId("build-check-screen-desk").textContent).toBe("desk ready in 900ms");
    expect(flags.getByTestId("build-check-screen-phone").dataset.ok).toBe("true");
    const stores = within(lines[0]);
    expect(stores.getByTestId("build-check-screen-desk").dataset.ok).toBe("true");
    expect(stores.getByTestId("build-check-screen-phone").dataset.ok).toBe("false");
    expect(stores.getByTestId("build-check-screen-phone").textContent).toBe("phone never ready");
    expect(stores.getByTestId("build-check-screen-phone").querySelector(".sr-only")).toBeNull();
    // A viewport that got ready keeps its time for a screen reader only.
    expect(
      stores.getByTestId("build-check-screen-desk").querySelector(".sr-only")?.textContent
    ).toBe(" ready in 900ms");
    expect(screen.getByTestId("build-check-screens-count").textContent).toBe(
      "3 screens · discovered · 2 left out"
    );
  });

  it("puts a screen's errors under that screen, and nowhere else", () => {
    renderList([SMOKE]);
    const [stores, flags, root] = screen.getAllByTestId("build-check-screen");
    expect(within(stores).getByTestId("build-check-screen-error").textContent).toBe(
      "phoneTypeError: x is undefined"
    );
    expect(within(flags).getByTestId("build-check-screen-error").textContent).toBe(
      "deskReferenceError: y is not defined"
    );
    expect(within(stores).getByTestId("build-check-screen-request")).toHaveTextContent(
      "403https://acme/fn/list"
    );
    // A write the checker was refused is counted, and named on hover.
    const refused = within(stores).getByTestId("build-check-screen-refused");
    expect(refused.textContent).toBe("1 write refused");
    expect(refused).toHaveAttribute("title", "POST /fn/save-plan");
    expect(within(root).queryByTestId("build-check-screen-error")).toBeNull();
    expect(within(root).queryByTestId("build-check-screen-refused")).toBeNull();
  });

  it("counts the requests to another site that were not sent, once each, named on hover", async () => {
    const font = "GET https://fonts.example.com/inter.woff2";
    renderList([
      row({
        detail: {
          findings: [],
          screens: [
            opened(
              "stores",
              { refused_off_origin: [font, "GET https://cdn.example.com/map.js"] },
              { refused_off_origin: [font] }
            ),
            opened("")
          ]
        }
      })
    ]);
    await open("smoke");
    const [stores, root] = screen.getAllByTestId("build-check-screen");
    const offOrigin = within(stores).getByTestId("build-check-screen-off-origin");
    expect(offOrigin.textContent).toBe("2 requests to another site not sent");
    expect(offOrigin).toHaveAttribute("title", `${font}\nGET https://cdn.example.com/map.js`);
    // A warning only: the screen is not a failing one.
    expect(stores.dataset.failing).toBe("false");
    expect(within(root).queryByTestId("build-check-screen-off-origin")).toBeNull();
  });

  it("says how many linked screens no check opened, and names them on a click", async () => {
    renderList([
      row({
        detail: {
          findings: [],
          discovered: false,
          screens: [opened("")],
          screens_unlisted: ["plans", "stores/12"],
          screens_unlisted_total: 5
        }
      })
    ]);
    await open("smoke");
    const unlisted = screen.getByTestId("build-check-screens-unlisted");
    expect(unlisted).not.toHaveAttribute("open");
    expect(unlisted.querySelector("summary")?.textContent).toBe("5 linked screens not checked");
    expect(Array.from(unlisted.querySelectorAll("li")).map((li) => li.textContent)).toEqual([
      "/plans",
      "/stores/12"
    ]);
    expect(unlisted).toHaveTextContent("and 3 more");
    expect(unlisted).toHaveTextContent("checks.screens");
  });

  it("says nothing of linked screens when the runner names none", () => {
    renderList([SMOKE]);
    expect(screen.queryByTestId("build-check-screens-unlisted")).toBeNull();
  });

  it("draws nothing extra for a server that sends none of the new fields", () => {
    renderList([
      row({ status: "failed", summary: "smoke failed", detail: { findings: [] } }),
      row({ kind: "production_smoke", label: "Production opens", status: "failed", detail: null })
    ]);
    expect(screen.getByTestId("build-check-evidence-smoke")).toBeEmptyDOMElement();
    expect(screen.queryByTestId("build-check-screens")).toBeNull();
    expect(screen.queryByTestId("build-check-new-errors")).toBeNull();
  });
});

describe("production smoke evidence", () => {
  it("lists the new errors first, then what it compared with, then every screen", () => {
    renderList(
      [
        row({
          kind: "production_smoke",
          label: "Production opens after promote",
          status: "failed",
          detail: {
            findings: [],
            baseline_at: "2026-11-22T09:58:00Z",
            screens: [opened("stores", { page_errors: ["TypeError: x"] }), opened("")],
            new_errors: [{ path: "stores", viewport: "desk", error: "TypeError: x" }]
          }
        })
      ],
      { env: "production" }
    );
    const evidence = screen.getByTestId("build-check-production-smoke");
    const order = Array.from(evidence.querySelectorAll("[data-testid]"), (el) =>
      el.getAttribute("data-testid")
    );
    expect(order.indexOf("build-check-new-errors")).toBeLessThan(
      order.indexOf("build-check-baseline")
    );
    expect(order.indexOf("build-check-baseline")).toBeLessThan(
      order.indexOf("build-check-screens")
    );
    expect(within(evidence).getByTestId("build-check-new-error").textContent).toBe(
      "/storesdeskTypeError: x"
    );
    expect(within(evidence).getByTestId("build-check-baseline")).toHaveTextContent(
      "Compared with production"
    );
  });

  it("says when there was nothing to compare with", () => {
    renderList([
      row({
        kind: "production_smoke",
        status: "failed",
        detail: { findings: [], baseline_at: null, screens: [opened("")], new_errors: [] }
      })
    ]);
    expect(screen.getByTestId("build-check-baseline").textContent).toBe("Nothing to compare with");
    expect(screen.queryByTestId("build-check-new-errors")).toBeNull();
    // An app that does not roll back by itself records no rollback, and none is drawn.
    expect(screen.queryByTestId("build-check-rollback")).toBeNull();
  });

  it("says first what the platform did to production: rolled back, or why not", () => {
    const failed = (rollback: unknown) =>
      row({
        kind: "production_smoke",
        status: "failed",
        detail: { findings: [], baseline_at: null, screens: [opened("")], new_errors: [], rollback }
      });
    const { unmount } = renderList([
      failed({
        rolled_back: true,
        to: { id: "row-o", build_id: "0ddba11" },
        at: "2026-11-22T10:02:00Z"
      })
    ]);
    const evidence = screen.getByTestId("build-check-production-smoke");
    expect(evidence.querySelector("[data-testid]")?.getAttribute("data-testid")).toBe(
      "build-check-rollback"
    );
    expect(screen.getByTestId("build-check-rollback").textContent).toBe(
      "Production was rolled back automatically to build 0ddba11"
    );
    unmount();
    renderList([
      failed({
        rolled_back: false,
        reason: "production served no other build before this one",
        at: "2026-11-22T10:02:00Z"
      })
    ]);
    expect(screen.getByTestId("build-check-rollback").textContent).toBe(
      "Not rolled back automatically: production served no other build before this one"
    );
  });
});

describe("changes evidence", () => {
  it("keeps its informs tag, and says how many differ and against which build", async () => {
    renderList([changes()]);
    expect(
      within(screen.getByTestId("build-check-changes")).getByTestId("build-check-informs")
    ).toHaveTextContent("informs");
    await open("changes");
    expect(screen.getByTestId("build-check-changes-count").textContent).toBe(
      "2 of 4 differ · production 60e2f19"
    );
  });

  it("opens the changed screens first, each as production then staging, with its share", async () => {
    renderList([changes()]);
    await open("changes");
    const differing = screen.getAllByTestId("build-check-change");
    expect(differing.map((li) => li.dataset.screen)).toEqual(["stores@desk", "stores@phone"]);
    expect(
      differing.map((li) => within(li).getByTestId("build-check-change-share").textContent)
    ).toEqual(["3.1%", "20%"]);
    await vi.waitFor(() =>
      expect(pictures(differing[0])).toEqual([
        ["shot-production-stores-desk.png", "ready"],
        ["shot-staging-stores-desk.png", "ready"]
      ])
    );
    expect(AppBuildChecksService.file).toHaveBeenCalledWith(
      "app-1",
      "row-s",
      "shot-production-stores-desk.png",
      { attempt: 2, environment: "staging" }
    );
    const img = within(differing[0]).getAllByRole("img")[0];
    expect(img).toHaveAttribute("alt", "Production, /stores desk");
    expect(img).toHaveAttribute("loading", "lazy");
    // Its own proportions: as wide as its frame, as tall as that makes it.
    expect(img.className).toContain("h-auto");
    expect(img.className).toContain("w-full");
    // The screens that did not change are one line, and none of their pictures is read.
    expect(screen.getByTestId("build-check-changes-same-toggle")).toHaveTextContent("2 the same");
    expect(fileNames().some((name) => name.includes("root"))).toBe(false);
  });

  it("reads the diff only when asked, in staging's place", async () => {
    renderList([changes()]);
    await open("changes");
    const [desk, phone] = screen.getAllByTestId("build-check-change");
    await vi.waitFor(() => expect(fileNames()).toContain("shot-staging-stores-desk.png"));
    expect(fileNames()).not.toContain("diff-stores-desk.png");
    // No diff was uploaded for the phone: no button to ask for one.
    expect(within(phone).queryByTestId("build-check-change-diff")).toBeNull();

    await userEvent.click(within(desk).getByTestId("build-check-change-diff"));
    await vi.waitFor(() =>
      expect(pictures(desk)).toEqual([
        ["shot-production-stores-desk.png", "ready"],
        ["diff-stores-desk.png", "ready"]
      ])
    );
    expect(within(desk).getByTestId("build-check-change-diff")).toHaveAttribute(
      "aria-pressed",
      "true"
    );
  });

  it("says a missing picture is missing, whether never named or gone, and shows no image", async () => {
    vi.mocked(AppBuildChecksService.file).mockImplementation(async (_a, _b, name) => {
      if (name === "shot-staging-stores-phone.png") throw notFound();
      return new Blob(["png"]);
    });
    renderList([changes()]);
    await open("changes");
    const phone = screen.getAllByTestId("build-check-change")[1];
    await vi.waitFor(() =>
      expect(pictures(phone)).toEqual([
        ["", "missing"],
        ["shot-staging-stores-phone.png", "missing"]
      ])
    );
    expect(within(phone).queryByRole("img")).toBeNull();
    expect(within(phone).getAllByText("Missing")).toHaveLength(2);
    // A name the check never gave is not asked for; a 404 is not asked for twice.
    expect(fileNames().filter((name) => name === "shot-staging-stores-phone.png")).toHaveLength(1);
  });

  it("says a read that failed could not load, not that the picture is missing", async () => {
    vi.mocked(AppBuildChecksService.file).mockRejectedValue(new Error("offline"));
    renderList([changes()]);
    await open("changes");
    const desk = screen.getAllByTestId("build-check-change")[0];
    await vi.waitFor(() => expect(pictures(desk)[0][1]).toBe("error"));
    expect(within(desk).getAllByText("Couldn't load")).toHaveLength(2);
  });

  it("reads a picture only once its frame is near the viewport", async () => {
    const seen: (() => void)[] = [];
    vi.stubGlobal(
      "IntersectionObserver",
      class {
        constructor(callback: (entries: { isIntersecting: boolean }[]) => void) {
          seen.push(() => callback([{ isIntersecting: true }]));
        }
        observe() {}
        disconnect() {}
      }
    );
    renderList([changes()]);
    await open("changes");
    const desk = screen.getAllByTestId("build-check-change")[0];
    expect(pictures(desk).map(([, state]) => state)).toEqual(["idle", "idle"]);
    expect(AppBuildChecksService.file).not.toHaveBeenCalled();
    seen[0]();
    await vi.waitFor(() => expect(pictures(desk)[0][1]).toBe("ready"));
    expect(fileNames()).toEqual(["shot-production-stores-desk.png"]);
  });

  it("opens a screen that did not change on a click", async () => {
    renderList([changes()]);
    await open("changes");
    await userEvent.click(screen.getByTestId("build-check-changes-same-toggle"));
    const same = within(screen.getByTestId("build-check-changes-same")).getAllByTestId(
      "build-check-change"
    );
    expect(same.map((li) => li.dataset.changed)).toEqual(["false", "false"]);
    expect(within(same[0]).queryByTestId("build-check-picture")).toBeNull();
    expect(within(same[0]).getByTestId("build-check-change-share").textContent).toBe("same");
    await userEvent.click(within(same[0]).getByTestId("build-check-change-toggle"));
    await vi.waitFor(() => expect(fileNames()).toContain("shot-staging-root-desk.png"));
  });

  it("draws no pictures for a row that was skipped, or one with none of the fields", async () => {
    renderList([
      changes({ findings: [] }, { status: "skipped", summary: "production serves nothing" })
    ]);
    await open("changes");
    expect(screen.getByTestId("build-check-evidence-changes")).toBeEmptyDOMElement();
    expect(AppBuildChecksService.file).not.toHaveBeenCalled();
  });

  it("shows its notes through the check note, marked as informing", async () => {
    const note = {
      author: "model",
      model: "model-test",
      text: "Look here first: the store dialog header."
    };
    renderList([changes(CHANGES, { notes: [note, { author: "model" }] as never })]);
    await open("changes");
    const notes = screen.getAllByTestId("build-check-note");
    expect(notes).toHaveLength(1);
    expect(notes[0]).toHaveTextContent("model-test · informs");
    expect(notes[0]).toHaveTextContent("Look here first: the store dialog header.");
  });
});

describe("Mark as seen", () => {
  const mark = () => screen.getByTestId("build-check-mark-seen");

  it("is offered whenever a screen changed and nobody has seen it, and sends the attempt", async () => {
    vi.mocked(AppBuildChecksService.markChangesSeen).mockResolvedValue();
    renderList([changes()]);
    await open("changes");
    expect(mark()).toHaveTextContent("Mark as seen");
    expect(isPrimary(mark())).toBe(false);
    await userEvent.click(mark());
    await vi.waitFor(() =>
      expect(AppBuildChecksService.markChangesSeen).toHaveBeenCalledWith("app-1", "row-s", 2)
    );
  });

  it("is the primary button, on a row already open, while the app holds Promote on it", () => {
    renderList([changes()], { seenRequired: true });
    // No click: the row starts open, since a person has to look.
    expect(screen.getByTestId("build-check-changes-toggle")).toHaveAttribute(
      "aria-expanded",
      "true"
    );
    expect(isPrimary(mark())).toBe(true);
    cleanup();
    // Otherwise it starts closed, as any check that passed does.
    renderList([changes()]);
    expect(screen.getByTestId("build-check-changes-toggle")).toHaveAttribute(
      "aria-expanded",
      "false"
    );
  });

  it("gives way to who saw it, once someone has", async () => {
    const seen = { by: "mars@oxy.tech", at: "2026-11-22T10:05:00Z" };
    renderList([changes({ ...CHANGES, seen })]);
    await open("changes");
    expect(screen.getByTestId("build-check-seen")).toHaveTextContent("Seen by mars@oxy.tech");
    expect(screen.queryByTestId("build-check-mark-seen")).toBeNull();
  });

  it("is not offered when nothing changed, or on a production attempt", async () => {
    const none = { ...CHANGES, changed: 0, screens: [shot("", "desk")] };
    renderList([changes(none)]);
    await open("changes");
    expect(screen.queryByTestId("build-check-mark-seen")).toBeNull();
    cleanup();
    renderList([changes()], { env: "production" });
    await open("changes");
    expect(screen.queryByTestId("build-check-mark-seen")).toBeNull();
  });

  it("says a 409 nothing_to_see beside the button", async () => {
    vi.mocked(AppBuildChecksService.markChangesSeen).mockRejectedValue(
      new AxiosError("refused", "ERR", undefined, undefined, {
        status: 409,
        data: { code: "nothing_to_see", message: "no screen changed" }
      } as AxiosResponse)
    );
    renderList([changes()]);
    await open("changes");
    await userEvent.click(mark());
    expect(await screen.findByTestId("build-check-seen-refused")).toHaveTextContent(
      "Nothing changed."
    );
  });
});
