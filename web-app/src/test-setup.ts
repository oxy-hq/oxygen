import "@testing-library/jest-dom/vitest";
import { vi } from "vitest";

// `persist-and-sync` (the IDE-branch and database-operation stores) restores a
// store from storage on a 100 ms timer, and the callback reads `document`. A
// jsdom file that finishes sooner has had its environment torn down by then, so
// the read throws `document is not defined` as an unhandled error and vitest
// fails the run with every test green. Which file trips it is worker timing —
// `useWorldModel.breakdown.test.ts` on one run, nothing on the next.
//
// No unit test is about cross-tab persistence (none seeds either store's
// storage key), so under test these are plain zustand stores: no timer, no
// `storage` listener left on `window`. `stores/persistedStores.test.ts` pins it.
vi.mock("persist-and-sync", () => ({
  persistNSync: (stateCreator: unknown) => stateCreator,
  clearStorage: () => {}
}));

// Radix observes its triggers (Tooltip, Popover, Select), and jsdom has no
// ResizeObserver — without this a component test renders but its trigger's
// activation never runs, which makes a test pass for the wrong reason rather
// than fail loudly. Shared here so the next file to render Radix does not have
// to rediscover it.
globalThis.ResizeObserver ??= class {
  observe() {}
  unobserve() {}
  disconnect() {}
} as unknown as typeof ResizeObserver;

// jsdom implements no CSS Object Model media queries, so `window.matchMedia` is
// simply absent. Anything that decides a layout from the viewport — the resizable
// docks, `useMediaQuery` — throws on first render without it, which reads as a
// component bug rather than a missing browser API.
//
// Defaults to "matches", i.e. the desktop branch: that is the layout the
// assertions in those tests are about, and a test that needs the narrow branch
// can override `window.matchMedia` for its own case.
if (typeof window !== "undefined" && !window.matchMedia) {
  window.matchMedia = ((query: string) => ({
    matches: true,
    media: query,
    onchange: null,
    addEventListener: () => {},
    removeEventListener: () => {},
    addListener: () => {},
    removeListener: () => {},
    dispatchEvent: () => false
  })) as unknown as typeof window.matchMedia;
}

// Vitest 4's jsdom environment copies a curated list of jsdom window properties
// onto the global, and `localStorage` / `sessionStorage` are not on it — so in a
// `@vitest-environment jsdom` file both read as `undefined` even though jsdom
// itself implements them fine. (Verify with
// `new JSDOM("", { url }).window.localStorage`, which is a real `Storage`.)
//
// This landed with the vitest 3 → 4 bump and it is not a niche gap: anything
// that remembers a user choice — the Ask dock's width, the onboarding
// "skip for now" flag — throws on first access, so the failure shows up as
// eighteen unrelated component tests dying in `beforeEach`.
//
// Borrowing a real `Storage` off a throwaway JSDOM rather than hand-rolling a
// Map-backed stand-in: the spec surface is larger than the four methods most
// call sites use (`key`, `length`, index access, the quota errors), and a
// half-shim is how a test passes against behaviour the browser does not have.
// The construction only happens in jsdom files — `window` is undefined in the
// default node environment — and jsdom is already loaded there, so it costs a
// module-cache hit.
if (typeof window !== "undefined" && !globalThis.localStorage) {
  const { JSDOM } = await import("jsdom");
  const storageHost = new JSDOM("", { url: window.location?.href ?? "http://localhost:3000/" });
  globalThis.localStorage = storageHost.window.localStorage;
  globalThis.sessionStorage = storageHost.window.sessionStorage;
}
