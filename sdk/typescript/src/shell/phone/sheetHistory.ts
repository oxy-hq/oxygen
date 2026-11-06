// What makes the phone's Back close a sheet. A sheet held in component state has
// no history entry, so Back (Android's button, iOS's swipe) moved the page under
// it, and with nothing behind it left the app. So each open sheet holds one entry
// on the same URL: Back steps off that entry, and only the top sheet closes.
//
// One coordinator for the whole page, not one per sheet, because a sheet cannot
// tell alone what a commit meant: one sheet closing as another opens needs no
// history at all, and StrictMode's mount/unmount/mount must not push and go back.
// So nothing happens until the commit is over, and then the history is moved to
// match what is open. `history.go` lands later, so only one move runs at a time.
//
// `depth` is how many of our entries are at and under the current one. Entries
// carry it on their own state, beside whatever the router keeps there, because
// after a traversal the state is the only way to know where we landed.
//
// A sheet the URL already names has its own entry and opts out (`historyEntry`).

const DEPTH = "oxyPhoneSheetDepth";
const RUN = "oxyPhoneSheetRun";
/** Longer than any real traversal; `go` that lands nowhere never fires popstate. */
const MOVE_TIMEOUT_MS = 1000;

/** This page load, so a mark left by an earlier one reads as no mark at all. */
const run = `${Date.now().toString(36)}-${Math.random().toString(36).slice(2)}`;
const open: Array<() => void> = [];
let depth = 0;
/** The URL under each of our entries: `urls[n]` is the page entry `n` sits on. */
let urls: string[] = [];
/** `history.length` after our last push; another value means the app pushed. */
let lengthAfterPush = 0;
let settleTimer: ReturnType<typeof setTimeout> | null = null;
let moveTimer: ReturnType<typeof setTimeout> | null = null;
/** A URL changed in place on an entry we are leaving, to put on the one we land on. */
let carry: string | null = null;
/** A sheet has closed since the last settle. */
let released = false;
/** A sheet has opened since the last settle. Both together are a swap. */
let opened = false;
/** The page is on its way out by `replacePageFrom`: nothing may go back under it. */
let departing = false;

type EntryState = Record<string, unknown> | null;

function depthOf(state: EntryState): number {
  if (state?.[RUN] !== run) return 0;
  const n = state[DEPTH];
  return typeof n === "number" ? n : 0;
}

function marked(state: EntryState, n: number): Record<string, unknown> {
  const { [DEPTH]: _depth, [RUN]: _run, ...rest } = state ?? {};
  return n > 0 ? { ...rest, [DEPTH]: n, [RUN]: run } : rest;
}

function push(want: number): void {
  // An app's `replaceState(null, …)` wipes the mark off the entry we stand on,
  // and Forward can land us on an old one. It is about to be somebody's floor.
  if (depthOf(window.history.state) !== depth) {
    window.history.replaceState(marked(window.history.state, depth), "", window.location.href);
  }
  while (depth < want) {
    urls.push(window.location.href);
    depth += 1;
    window.history.pushState(marked(window.history.state, depth), "", window.location.href);
  }
  lengthAfterPush = window.history.length;
}

function move(steps: number): void {
  moveTimer = setTimeout(() => {
    moveTimer = null;
    carry = null;
    depth = Math.min(depth, open.length);
    urls.length = Math.min(urls.length, depth);
    settle();
  }, MOVE_TIMEOUT_MS);
  window.history.go(steps);
}

function abandon(): void {
  depth = 0;
  urls = [];
}

/** Sheets closed without Back: take their entries away, keeping what the URL says now. */
function leave(want: number): void {
  const before = urls[want];
  const here = window.location.href;
  if (before !== undefined && here !== before) {
    if (window.history.length !== lengthAfterPush) {
      // The app pushed, so a navigation from inside the sheet: going back would
      // undo it, and our entries can no longer be removed. Give them up: Back onto
      // one steps straight down off it. A sheet still open gets a new entry on top.
      abandon();
      if (open.length > 0) push(open.length);
      return;
    }
    // A filter, or a sheet the URL names by replacing: the entry's URL changed
    // in place. It goes down with us, or the page would land on the old one (and
    // a URL that names the sheet would open it again).
    carry = here;
  }
  move(want - depth);
}

function settle(): void {
  if (moveTimer !== null) return;
  const want = open.length;
  const swapped = released && opened && want === depth && want > 0;
  released = false;
  opened = false;
  if (want > depth) push(want);
  else if (want < depth) {
    if (!departing) leave(want);
  }
  // One sheet closed as another opened and the URL moved in place (Store Ops' "View
  // submission" switches module): the page has gone with it, so the entry below
  // must go too, or Back would land on a page that is no longer showing. `leave`
  // takes the entry down with the URL, and the settle after it gives the new sheet
  // a fresh one. A pushed URL gets the same treatment `leave` gives any other.
  else if (swapped && urls[want - 1] !== window.location.href) leave(want - 1);
}

function schedule(): void {
  if (settleTimer !== null) return;
  settleTimer = setTimeout(() => {
    settleTimer = null;
    settle();
  }, 0);
}

const withoutHash = (url: string) => url.split("#")[0];

/**
 * Browsers fire popstate for a jump to a `#` link too, and that is no step back:
 * it puts a new entry with no state on top. A real Back has the same look when it
 * lands on an entry an app pushed, so tell them apart by where the jump can land:
 * the history one longer than we left it, or the URL we were on but for its hash
 * (a jump can overwrite an entry a Back left ahead of us, so the length can hold).
 * A Back down onto the page's own entry always lands on `urls[0]`.
 */
function jumpedToHash(before: number, state: EntryState): boolean {
  const here = window.location.href;
  if (before === 0 || state != null || here === urls[0]) return false;
  const under = urls[before - 1];
  return (
    window.history.length === lengthAfterPush + 1 ||
    (under !== undefined && withoutHash(under) === withoutHash(here))
  );
}

function onPopState(): void {
  // A move of ours landing closes nothing: its sheets have gone, and one open now
  // opened after it set off, and gets its entry from `schedule` below.
  const ours = moveTimer !== null;
  if (moveTimer !== null) clearTimeout(moveTimer);
  moveTimer = null;
  // The entry's state, not the event's: a tap on a link to the URL already showing
  // fires popstate with a null state and leaves the entry as it was (measured in
  // Chromium and WebKit). On a real traversal the two are the same.
  const state = window.history.state as EntryState;
  if (!ours && jumpedToHash(depth, state)) {
    // The entries under it can no longer be removed, as when an app pushes: the
    // sheets stay, and get new entries on top of the jump from `schedule`.
    abandon();
    schedule();
    return;
  }
  const landed = depthOf(state);
  depth = landed;
  urls.length = Math.min(urls.length, landed);
  if (carry !== null && landed <= open.length) {
    window.history.replaceState(window.history.state, "", carry);
    carry = null;
  }
  // Top first.
  if (!ours) for (const close of open.slice(landed).reverse()) close();
  // A sheet that did not close (unsaved work asking first) gets its entry back.
  // An entry whose sheet has gone (Forward onto it, or Back onto one left under a
  // navigation) steps down off it, so Back never lands on a step that does nothing.
  schedule();
}

// Listening from load, for the same reason `layers` does: a carried URL must be in
// place before a router's popstate listener reads it, and a render that catches
// the old one would open the sheet again. Chromium runs a window's listeners in
// the order they were added whatever the phase, so capture alone is not enough:
// this one has to come before any an app adds, which is in an effect at the
// earliest. Away from our own entries it finds nothing to do.
if (typeof window !== "undefined") {
  window.addEventListener("popstate", onPopState, true);
  // Back to a page kept whole in the browser's cache: this module wakes as it was
  // left, mid-departure, on an entry that may not be the one it left from.
  window.addEventListener("pageshow", (event) => {
    if (!event.persisted) return;
    departing = false;
    depth = depthOf(window.history.state);
    urls.length = Math.min(urls.length, depth);
    schedule();
  });
}

/**
 * Leave for another document from inside an open sheet, its entry becoming the new
 * page: `[page, sheet]` ends as `[page, next]`, so Back goes to the page. Stepping
 * down first would race the load (a traversal cancels a navigation under way), and
 * leaving the entry in place would be a Back that shows no change.
 */
export function replacePageFrom(href: string): void {
  departing = true;
  window.location.replace(href);
}

/** While a sheet is open: hold a history entry for it. Returns the release. */
export function holdHistoryEntry(close: () => void): () => void {
  // A departure that never unloaded (a download, a 204): this sheet takes over
  // the entry it left behind, and closes normally from here on.
  departing = false;
  open.push(close);
  opened = true;
  schedule();
  return () => {
    const at = open.lastIndexOf(close);
    if (at >= 0) open.splice(at, 1);
    released = true;
    schedule();
  };
}
