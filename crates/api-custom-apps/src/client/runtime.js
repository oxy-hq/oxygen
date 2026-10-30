/*
 * Oxy custom-app client runtime — injected into every served HTML document,
 * next to `window.__OXY_APP__`.
 *
 * Its jobs, all of which exist so an app author has to do nothing to get them:
 *
 *  1. **Register the platform service worker** (`__oxy/sw.js`) at the app root,
 *     which is what makes a repeat load of a published bundle near-instant.
 *  2. **Report platform health** — did the app mount (`oxy-app-ready`), and did
 *     it throw (`oxy-error`).
 *  3. **Instrument the app by default** — SPA pageviews, Core Web Vitals and
 *     engagement time, posted to `__oxy/beacon`.
 *  4. **Notice a new build** in a tab left open for hours or days, and — in
 *     `auto` mode only — reload onto it at a safe moment. See § *updates*.
 *  5. **Keep same-tab navigation honest.** Report an open the server could not
 *     see (a page shown from a prefetch), and keep an app's boot-time URL
 *     rewrites out of the Back history.
 *
 * ## Two categories, one flag
 *
 * `analytics: false` in `oxy-app.json` silences (3) and nothing else. (2) is
 * platform health and is never opt-out-able, for the same reason the
 * server-side view row never was: an operator cannot answer "is this app up"
 * by editing someone else's bundle, and an app that is down is exactly the app
 * least likely to have opted in.
 *
 * The split is drawn where it is because health costs no privacy. `app-ready`
 * is a boolean — it carries no path and says only that the bundle rendered
 * something. Errors cross the line as *constructor names and counts* only;
 * their message, stack and screen path are free text from the page and follow
 * the opt-out with everything else. Neither exempt signal describes what a
 * viewer did. `oxy-pageview` does, which is why it sits on the analytics side
 * despite being useful here: the server-side view row is a sufficient
 * denominator for the white-screen signal without it.
 *
 * ## What is and is not collected
 *
 * Analytics events carry no free-form strings from the page: no query strings,
 * no element selectors, no DOM text. Paths are same-origin and truncated.
 *
 * **Errors are the exception, and it is deliberate.** Since 2026-09 an error
 * report carries its message and stack as well as its name, because names and
 * counts alone made a white-screened app report `{TypeError: 3}` and nothing
 * anyone could act on. That text goes to a *separate* server-side table with
 * shorter retention behind the app-admin gate — never into the analytics rows
 * this runtime otherwise produces — and it is sent only when the app has NOT
 * opted out. Bounds live in the error section below: dedup per session by
 * stack, a cap per flush, truncation before the wire.
 */
(function () {
  "use strict";
  var app = window.__OXY_APP__;
  if (!app || typeof app !== "object") return;

  var base = (app.basePath || "/").replace(/\/*$/, "/");

  // ── service worker ────────────────────────────────────────────────────────
  //
  // Scope is decided HERE, not by the server, because it differs per surface and
  // the HTML is memoized per build rather than per surface:
  //
  //   - Admin host: the page is at `<base>…`, so the worker scopes to `base`
  //     and does not touch the rest of the admin origin.
  //   - Custom-app subdomain: the page is at `/` while the bundle's assets keep
  //     the `<base>` prefix (the host dispatcher passes those through rather
  //     than double-prefixing). Scope must be `/` or the worker never controls
  //     the page; `?base=` tells it where the assets actually live.
  //
  // "Does the current document sit under `base`?" distinguishes the two exactly,
  // needs nothing from the server, and cannot disagree with what the browser is
  // showing. The origin still has the final say: it only sends the
  // `Service-Worker-Allowed` header widening scope to `/` on a subdomain host,
  // so a worker on the admin origin cannot claim the whole SPA even if this
  // computed the wrong answer.
  //
  // `isSecureContext` covers https and localhost, which is exactly where the API
  // exists. Registered after load so it never competes with the critical path
  // for the connection it is meant to make unnecessary.
  if (app.serviceWorker !== false && "serviceWorker" in navigator && window.isSecureContext) {
    var scope = location.pathname.indexOf(base) === 0 ? base : "/";
    var script = base + "__oxy/sw.js?base=" + encodeURIComponent(base);
    var register = function () {
      navigator.serviceWorker.register(script, { scope: scope }).catch(function (err) {
        // Not silent. A worker that will not register costs only the cold path,
        // but "the fast path quietly does not exist here" is exactly the kind of
        // thing that goes unnoticed for a quarter.
        //
        // The known case is the **org-subdomain** surface (`<org>.…/a/<slug>/`):
        // the document lives under `/a/<slug>/` while the bundle's assets keep
        // the `/customer-apps/<org>/<slug>/` prefix, so `scope` computes to `/`
        // and the origin — which only widens `Service-Worker-Allowed` to `/` for
        // a custom-app subdomain — refuses it. Failing closed is right: a `/`
        // scope there would let one app's worker intercept the whole org
        // product origin, SPA included. See `customer-apps-performance.md`
        // § *The client plane* for what a real fix would need.
        if (window.console && console.info) {
          console.info(
            "[oxy] service worker not registered for scope " + scope + " — app runs on the " +
              "network path. " + (err && err.name ? err.name : "registration failed")
          );
        }
      });
    };
    if (document.readyState === "complete") register();
    else window.addEventListener("load", register);
  }

  // ── the opt-out split ─────────────────────────────────────────────────────
  //
  // `analytics: false` used to return here and silence the client runtime
  // entirely. It now silences the *author's product analytics* only —
  // pageviews, web vitals, engagement. Two signals stay on for every app:
  //
  //   `oxy-app-ready`  did this app actually mount?
  //   `oxy-error`      did it throw? (counts only — the message/stack detail
  //                    follows the opt-out, see `noteError`)
  //
  // Those are platform health, not product analytics, and they are exempt for
  // the same reason the server-side view row always was: an operator cannot
  // fix "is this app up" by editing someone else's bundle. They also say
  // nothing about what a viewer did — `app-ready` is a boolean and carries no
  // path, and errors are names and counts. `oxy-pageview` deliberately stays
  // opt-out-able: a list of screen paths IS product analytics, and the
  // server-side view row is a sufficient denominator without it.
  var analyticsEnabled = app.analytics !== false;

  // ── beacon queue ──────────────────────────────────────────────────────────
  var ENDPOINT = base + "__oxy/beacon";
  /** Hard cap per flush — mirrors the server's own limit, so a burst is
   *  dropped here rather than rejected there. */
  var MAX_BATCH = 20;
  var FLUSH_MS = 15000;
  var queue = [];
  var flushTimer = null;
  // Why a batch may wait: the page is still prerendering, or the open it
  // reports is on its way. See "an open the server could not see" below.
  var prerendering = !!document.prerendering;
  var openInFlight = false;

  /** `first` puts the event at the head of the queue, past the cap: the open a
   *  page reports must ride the next batch whatever is already waiting. */
  function push(name, payload, first) {
    var event = { n: name, t: Date.now(), p: payload || {} };
    if (first) queue.unshift(event);
    else if (queue.length < MAX_BATCH) queue.push(event);
    else return;
    if (!flushTimer) flushTimer = setTimeout(flush, FLUSH_MS);
  }

  /** Send one batch. Returns the request when it went by `fetch`. */
  function flush(useBeacon) {
    if (flushTimer) {
      clearTimeout(flushTimer);
      flushTimer = null;
    }
    if (!queue.length) return;
    // A prerendered page nobody showed was never opened, so nothing it queued
    // is sent, not even on unload. While a reported open is on its way, other
    // batches wait for the session cookie its response sets; an unloading page
    // sends anyway, because a batch in its own session beats a lost one.
    if (prerendering || (openInFlight && !useBeacon)) return;
    var body = JSON.stringify({ v: 1, events: queue.splice(0, MAX_BATCH) });
    // `sendBeacon` is the only transport that survives an unload; `keepalive`
    // fetch is the one that reports failures. Use each where it fits.
    if (useBeacon && navigator.sendBeacon) {
      try {
        navigator.sendBeacon(ENDPOINT, new Blob([body], { type: "application/json" }));
        return;
      } catch (e) {
        /* fall through to fetch */
      }
    }
    try {
      return fetch(ENDPOINT, {
        method: "POST",
        headers: { "content-type": "application/json" },
        body: body,
        credentials: "same-origin",
        keepalive: true
      }).catch(function () {});
    } catch (e) {
      /* analytics must never surface to the app */
    }
  }

  /** Same-origin path only, query and hash dropped, length-bounded. The path is
   *  the whole signal ("which screen"); a query string is where an app puts
   *  record ids and filter values, i.e. the part most likely to be sensitive
   *  and least likely to be useful grouped. */
  function currentPath() {
    var p = location.pathname || "/";
    if (base !== "/" && p.indexOf(base) === 0) p = "/" + p.slice(base.length);
    return p.slice(0, 256);
  }

  // ── an open the server could not see (never opt-out-able) ─────────────────
  //
  // The server records a view when it serves a page, except a speculative fetch,
  // which it skips (`is_speculative_request`) because a hover is not an open.
  // When the person then does open the app, the browser may show the page it
  // already fetched without asking the server again, and the open would go
  // unrecorded. Oxygen prefetches apps on hover from the home page and from the
  // dock (`prefetchApp`, `OxyShell`), so this is the common path, not an edge.
  //
  // The navigation timing entry says which happened: `deliveryType` is
  // `navigational-prefetch` for a Speculation Rules prefetch, and
  // `activationStart` is non-zero for a prerender. In either case this page
  // reports its own open, and the beacon writes the same view row the server
  // would have. (Oxygen only prefetches where `deliveryType` exists, so that a
  // reused prefetch can always be told apart.)
  //
  // The page's session starts there too. The speculative response set no
  // tracking cookie, and the beacon that carries the open sets it, so the open
  // goes at the head of the next batch and the server files that whole batch
  // under the open's session. Later batches wait for its response, or for
  // OPEN_HOLD_MS if it never comes, so they carry the cookie rather than each
  // starting a session of its own. A prerender queues what it has to say (its
  // `app-ready`, say) until it is shown, and it rides with the open.
  var OPEN_HOLD_MS = 5000;
  function reportSpeculativeOpen() {
    prerendering = false;
    try {
      var nav = performance.getEntriesByType && performance.getEntriesByType("navigation")[0];
      if (!nav || (nav.deliveryType !== "navigational-prefetch" && !(nav.activationStart > 0))) {
        flush(false);
        return;
      }
      push("oxy-view", { referrer: document.referrer || "" }, true);
      var sent = flush(false);
      if (!sent) return;
      openInFlight = true;
      var release = function () {
        if (!openInFlight) return;
        openInFlight = false;
        flush(false);
      };
      sent.then(release, release);
      setTimeout(release, OPEN_HOLD_MS);
    } catch (e) {}
  }
  // A prerendered page runs before anyone has opened it; it is an open only
  // once it is shown.
  if (document.prerendering) {
    document.addEventListener("prerenderingchange", reportSpeculativeOpen, { once: true });
  } else {
    reportSpeculativeOpen();
  }

  // ── app-ready (health signal, never opt-out-able) ─────────────────────────
  //
  // THE white-screen detector, and the reason a status code is not enough. A
  // custom-app host answers 200 with the SPA shell for every path — so a bundle
  // that throws on boot, or whose main chunk 404s, produces a perfect 200 and a
  // blank page. The server cannot tell those apart. The browser can: pair
  // "a shell was served" (the server-side view row, which no app can opt out
  // of) with "the app mounted", and the *absence* of the second is the outage.
  //
  // Absence is the signal, so this deliberately never sends a negative. A
  // "failed to mount" beacon would need the page to survive long enough to send
  // it, which is exactly what a hard boot failure does not do.
  var readySent = false;
  function markReady(how) {
    if (readySent) return;
    readySent = true;
    // `build` is the build this document runs, so the operator view can count
    // sessions still on an older one. An id, not a path: still health.
    push("oxy-app-ready", { how: how, build: app.buildId || "" });
    // Sent immediately rather than on the 15s timer: a boot failure that takes
    // the tab with it must not also take the proof that boot succeeded.
    flush(false);
  }

  // Explicit opt-in for an app that knows better than the heuristic — a bundle
  // that renders a deliberate splash, or mounts into a shadow root.
  window.__oxyAppReady = function () {
    try {
      markReady("explicit");
    } catch (e) {}
  };

  /** Has anything with layout rendered? Bounded scan: it returns on the first
   *  laid-out element, so a mounted app costs a handful of iterations, and an
   *  unmounted one has almost no DOM to walk. The cap is for the pathological
   *  middle — a huge DOM of zero-height nodes — where this must not become the
   *  jank it is measuring. */
  function looksMounted() {
    var root = document.body;
    if (!root) return false;
    var kids = root.querySelectorAll("*");
    var limit = Math.min(kids.length, 200);
    for (var i = 0; i < limit; i++) {
      var el = kids[i];
      var tag = el.tagName;
      if (tag === "SCRIPT" || tag === "STYLE" || tag === "LINK" || tag === "NOSCRIPT") continue;
      try {
        if (el.getBoundingClientRect().height > 0) return true;
      } catch (e) {}
    }
    return false;
  }

  // Polled rather than observed: MutationObserver fires on the first DOM write,
  // which for a framework is the empty root container — earlier than "mounted"
  // and it would call a white screen healthy. These checks bracket the range a
  // real app boots in; past the last one it is not booting.
  function checkReady() {
    if (readySent) return;
    if (looksMounted()) markReady("auto");
  }
  [0, 1000, 3000, 10000].forEach(function (delay) {
    setTimeout(checkReady, delay);
  });
  if (document.readyState === "complete") checkReady();
  else window.addEventListener("load", checkReady);

  // ── SPA pageviews ─────────────────────────────────────────────────────────
  var lastPath = currentPath();
  var routeCount = 1;
  var pageStart = Date.now();
  var maxScroll = 0;

  function onRouteChange() {
    var next = currentPath();
    if (next === lastPath) return;
    // The screen being left is the one whose engagement just ended.
    emitEngagement("route");
    lastPath = next;
    routeCount++;
    pageStart = Date.now();
    maxScroll = 0;
    if (analyticsEnabled) push("oxy-pageview", { path: next, kind: "spa" });
  }

  // Hooks the update section (below) installs when it is shipped and on.
  var onRoute = function () {};
  var onChunk = null;

  // ── navigations the person did not make ───────────────────────────────────
  //
  // An app rewrites its own URL as it boots: a router's default redirect, a
  // screen writing its filters into the query string. Done with `pushState`,
  // that adds a history entry nobody asked for, so Back from the app's first
  // screen lands on its bare URL, which redirects forward again. It reads as a
  // Back button that does nothing, and it is what you get from the home page and
  // the dock, which open an app in the same tab. Chrome skips such an entry on
  // Back, but only until the page is first activated; Safari and Firefox never
  // do.
  //
  // So until the person has done something on this page, a push is made a
  // replace. `userActivation.hasBeenActive` is the browser's own record of that
  // (a click, a key, a tap) and stays true for the life of the document; the
  // listeners stand in where it is missing. A push after that is the person's,
  // and is left alone.
  //
  // Chromium gets the replace too: by the time the person presses Back they
  // have clicked something, so Chrome has stopped skipping the entry, and Back
  // would land on the bare URL and redirect forward again.
  //
  // The price is an app that pushes on purpose before any interaction (a
  // dialog route opened on load and closed with `history.back()`, a kiosk's
  // Back trap): the entry does not exist, so it would leave the app instead.
  // Such an app says so with `<meta name="oxy-history" content="native">` in
  // its `index.html`, and its pushes are left alone.
  var acted = false;
  function hasActed() {
    var ua = navigator.userActivation;
    return acted || !!(ua && ua.hasBeenActive);
  }
  function keepsEarlyPushes() {
    var meta = document.querySelector('meta[name="oxy-history"]');
    return !!meta && meta.getAttribute("content") === "native";
  }
  if (!navigator.userActivation) {
    ["pointerdown", "keydown"].forEach(function (type) {
      window.addEventListener(
        type,
        function () {
          acted = true;
        },
        { capture: true, once: true }
      );
    });
  }

  // History is patched rather than polled: a router calls `pushState`
  // synchronously and there is no event for it, so the alternatives are a
  // patch or a timer that is always either late or wasteful.
  var nativeReplaceState = history.replaceState;
  ["pushState", "replaceState"].forEach(function (method) {
    var original = history[method];
    if (typeof original !== "function") return;
    history[method] = function () {
      var isPush = method === "pushState";
      // Named for what it is, and not `push`, which is the beacon's.
      var personsPush = isPush && hasActed();
      var asReplace = isPush && !personsPush && !keepsEarlyPushes();
      var result = (asReplace ? nativeReplaceState : original).apply(this, arguments);
      try {
        onRouteChange();
        // Only a PUSH is a safe moment to reload onto a new build: a router
        // rewrites the URL with `replaceState` mid-render (query sync, redirects),
        // and reloading under that would interrupt the screen being drawn. A
        // push before the person has done anything is that same boot-time
        // rewrite, whether or not it was made a replace above, so it counts as
        // one.
        onRoute(personsPush);
      } catch (e) {}
      return result;
    };
  });
  window.addEventListener("popstate", onRouteChange);
  window.addEventListener("popstate", function () {
    onRoute(true);
  });
  window.addEventListener("hashchange", onRouteChange);

  // ── engagement ────────────────────────────────────────────────────────────
  window.addEventListener(
    "scroll",
    function () {
      var doc = document.documentElement;
      var height = Math.max(1, doc.scrollHeight - window.innerHeight);
      var pct = Math.round((Math.min(height, window.scrollY) / height) * 100);
      if (pct > maxScroll) maxScroll = pct;
    },
    { passive: true }
  );

  function emitEngagement(reason) {
    if (!analyticsEnabled) return;
    var ms = Date.now() - pageStart;
    // Sub-second views are router churn (a redirect, a guard bouncing), not a
    // human looking at a screen. Recording them would put the mode of the
    // duration distribution at zero.
    if (ms < 1000) return;
    // Closing the window as we report it. An ordinary desktop navigation fires
    // `visibilitychange → hidden` AND then `pagehide`, and both call
    // `finalize()`; without this the same stretch of time is pushed twice and
    // every session's engagement total doubles. `emitVitals` has `vitalsSent`
    // and `emitErrors` empties its map — this is the third one, and it was the
    // one missing a guard. Advancing `pageStart` rather than setting a flag is
    // what also makes the tab-away/tab-back case add up instead of overlapping.
    pageStart = Date.now();
    push("oxy-engagement", {
      path: lastPath,
      ms: Math.min(ms, 6 * 60 * 60 * 1000),
      scroll: maxScroll,
      reason: reason
    });
  }

  // ── Core Web Vitals ───────────────────────────────────────────────────────
  // Hand-rolled rather than pulled from `web-vitals`: this script is inlined
  // into every HTML response, so its size is paid on every navigation of every
  // app. The numbers below are the ones the console's release timeline reads
  // per screen and per build, and they are ~40 lines of PerformanceObserver.
  //
  // Sent once per document, at 10 s or on the first hide, so INP and CLS cover
  // that much of the page's life and no more.
  var vitals = {};
  // The screen the document landed on, kept before any route change: TTFB,
  // FCP and LCP belong to it, and the emit runs after the operator may have
  // moved on. Query and hash already dropped (`currentPath`).
  var landingPath = lastPath;
  var nav =
    analyticsEnabled &&
    performance.getEntriesByType &&
    performance.getEntriesByType("navigation")[0];
  if (nav) {
    vitals.ttfb = Math.round(nav.responseStart);
    if (nav.domContentLoadedEventEnd) vitals.dcl = Math.round(nav.domContentLoadedEventEnd);
  }

  // No-op when the author opted out, so an opted-out app does not pay for
  // observers whose output would be discarded at emit time anyway.
  function observe(type, handler, opts) {
    if (!analyticsEnabled) return null;
    try {
      var po = new PerformanceObserver(function (list) {
        list.getEntries().forEach(handler);
      });
      po.observe(Object.assign({ type: type, buffered: true }, opts || {}));
      return po;
    } catch (e) {
      return null;
    }
  }

  observe("paint", function (entry) {
    if (entry.name === "first-contentful-paint") vitals.fcp = Math.round(entry.startTime);
  });
  observe("largest-contentful-paint", function (entry) {
    // LCP is "largest so far" — every entry supersedes the last.
    vitals.lcp = Math.round(entry.startTime);
  });
  observe("layout-shift", function (entry) {
    if (entry.hadRecentInput) return;
    // Stored as CLS × 1000, i.e. an integer of milli-units: the payload column
    // is read by humans and by charts, and a float that arrives as 0.0500000001
    // groups badly and reads worse. `cls: 70` means 0.07 — the "good" threshold
    // is 100. Accumulated by dividing back out first so the rounding does not
    // compound across shifts.
    vitals.cls = Math.round(((vitals.cls || 0) / 1000 + entry.value) * 1000);
  });
  observe("event", function (entry) {
    // INP approximated by the worst interaction latency seen before the emit
    // (the first 10 s, or until the first hide). The real metric is a high
    // percentile of all interactions; the max is the number that actually gets
    // acted on, and it needs no client-side histogram.
    var d = Math.round(entry.duration);
    if (!vitals.inp || d > vitals.inp) vitals.inp = d;
  }, { durationThreshold: 40 });

  var vitalsSent = false;
  function emitVitals() {
    if (!analyticsEnabled) return;
    if (vitalsSent) return;
    vitalsSent = true;
    // `path` and `build` attribute the numbers: which screen, on which build
    // (`app_builds.id`, as `oxy-app-ready` sends it). Without `build` a reader
    // could only guess the build from the session, which can span two.
    if (!Object.keys(vitals).length) return;
    var attributed = Object.assign({ path: landingPath, build: app.buildId || "" }, vitals);
    push("oxy-web-vitals", attributed);
  }

  // ── errors ────────────────────────────────────────────────────────────────
  //
  // Counts by name AND, since 2026-09, the message and stack. The old rule was
  // "never messages or stacks", because a thrown value routinely carries a row
  // id, a SQL fragment, or a user's own input. That rule also made a
  // white-screened app report `{TypeError: 3}` and nothing anyone could act on,
  // so it was traded deliberately (design doc §5.1): detail goes to a SEPARATE
  // server-side table with 30-day retention behind the app-admin gate, while
  // the counts keep feeding the 90-day Activity rollup as before. One event,
  // two sinks.
  //
  // Three bounds, because an error channel is the one place a page can shout:
  //
  //  1. **Dedup per session by stack.** A fault in a render loop fires
  //     thousands of times; it is one problem. The first occurrence of each
  //     distinct (name, stack) carries detail, the rest only increment counts.
  //  2. **A cap per flush**, independent of the batch cap, so a page throwing
  //     five different errors cannot spend the whole batch on itself and starve
  //     `app-ready` or web vitals.
  //  3. **Truncation** of both message and stack, here rather than only on the
  //     server, so the bytes are never put on the wire in the first place.
  var errors = {};
  var errorDetails = [];
  var seenStacks = {};
  var MAX_ERROR_DETAILS = 5;
  var MAX_MESSAGE_CHARS = 1000;
  var MAX_STACK_CHARS = 8000;

  /** Origin-stripped so the same fault hashes identically on the subpath and
   *  subdomain surfaces — otherwise one bug groups as two. */
  function normaliseStack(stack) {
    return String(stack || "").replace(/https?:\/\/[^/\s)]+/g, "");
  }

  /** djb2, base36. Not a cryptographic hash and does not need to be — it is a
   *  grouping key, and a collision merges two error groups in a debugging UI
   *  rather than losing data. */
  function hashString(input) {
    var h = 5381;
    for (var i = 0; i < input.length; i++) {
      h = ((h << 5) + h + input.charCodeAt(i)) | 0;
    }
    return (h >>> 0).toString(36);
  }

  function noteError(name, message, stack, kind, traceId) {
    var key = String(name || "Error").slice(0, 40);
    errors[key] = (errors[key] || 0) + 1;

    // Counts are a health signal and ride past the opt-out. The DETAIL —
    // message, stack, screen path — does not: it is free text from the page,
    // which is exactly what the exemption's argument says health signals never
    // carry. An app with `analytics: false` still reports THAT it threw, so
    // availability is unaffected; it just stops shipping what the text said.
    if (!analyticsEnabled) return;

    var normalised = normaliseStack(stack);
    var hash = hashString(key + "|" + normalised);
    // Already reported this fault this session, or the flush is full: the
    // count above still moves, so nothing is lost that the rollup measures.
    if (seenStacks[hash] || errorDetails.length >= MAX_ERROR_DETAILS) return;
    seenStacks[hash] = 1;
    var detail = {
      n: key,
      m: String(message == null ? "" : message).slice(0, MAX_MESSAGE_CHARS),
      s: String(stack || "").slice(0, MAX_STACK_CHARS),
      h: hash,
      k: kind,
      p: lastPath
    };
    // The SDK stamps `traceId` on an invoke that failed, so an uncaught
    // rejection from `useFunction` names the server-side trace it came from.
    if (typeof traceId === "string" && /^[0-9a-f]{32}$/.test(traceId)) detail.t = traceId;
    errorDetails.push(detail);
  }

  window.addEventListener("error", function (e) {
    var err = e && e.error;
    noteError(
      err && err.name,
      (err && err.message) || (e && e.message),
      err && err.stack,
      "error",
      err && err.traceId
    );
  });
  window.addEventListener("unhandledrejection", function (e) {
    var r = e && e.reason;
    // A rejection can carry anything — an Error, a string, a response object.
    // Only an Error has `name`/`stack`; for the rest the type is the most
    // honest name available, and `String(r)` is the only message there is.
    var note = function () {
      noteError(
        r && r.name ? r.name : typeof r,
        r && r.message ? r.message : safeString(r),
        r && r.stack,
        "unhandledrejection",
        r && r.traceId
      );
    };
    // A lazy chunk that failed to load is usually version skew, not a bug —
    // see `recoverChunk`. It is reported as before unless that recovers it.
    if (!(onChunk && onChunk(r, note))) note();
  });

  /** `String(x)` throws on a proxy with a hostile `toString`, and an analytics
   *  path must never be the thing that breaks a page. */
  function safeString(value) {
    try {
      return String(value);
    } catch (e) {
      return "";
    }
  }

  function emitErrors() {
    var names = Object.keys(errors);
    if (!names.length) return;
    // `b` is the build that SERVED this document (`buildId` on the runtime
    // config), not the build that happens to be published when the beacon
    // lands — by then the app may have been re-published, and a stack
    // attributed to the wrong build cannot be resolved against a source map.
    // The client is the only party that knows which build it is running.
    push("oxy-error", {
      counts: errors,
      // The screen path is product analytics wherever it appears — the header
      // above says so about `oxy-pageview`, and a crashing SPA's error paths
      // are a subset of that same list. Gating `noteError`'s detail but not
      // this left the exempt event still shipping a path off-page once per
      // error batch. Availability reads the event's presence and `outcome`,
      // never `route`, so blanking it costs the health signal nothing.
      path: analyticsEnabled ? lastPath : "",
      build: app.buildId || "",
      details: errorDetails
    });
    errors = {};
    errorDetails = [];
    // `seenStacks` is deliberately NOT cleared: dedup is per session, not per
    // flush. Clearing it would re-report the same fault every 15 seconds for as
    // long as the page stays open, which is the behaviour the cap exists to
    // prevent.
  }

  // @oxy-updates:begin
  // ── updates ───────────────────────────────────────────────────────────────
  //
  // Everything between the two `@oxy-updates` marker lines is cut from the
  // inlined script when the mode is `off` (`oxy-api-custom-apps` `client.rs`), so the
  // rest of this file must reach it only through `onRoute` / `onChunk`.
  //
  // A custom app is a single-page app: a tab runs the build it loaded until the
  // document reloads. An iPad left on a checklist, a kiosk or an installed PWA
  // can therefore run an old build for days, and its lazy chunks 404 once a
  // publish replaces them. This section closes that gap without the app doing
  // anything. Modes come from the server (`app.updates`, see
  // `custom_apps_updates.rs`):
  //
  //   off      nothing below runs (and it is not even shipped).
  //   observe  DETECT + ANNOUNCE + stale-chunk recovery; never reloads otherwise.
  //   auto     observe, plus APPLY at a safe moment.
  //   prompt   observe, with the SDK offering an "Update ready" control; turns
  //            into auto 24 h after detection so an unattended kiosk converges.
  //
  // DETECT: poll `__oxy/build` every ~5 min while visible, and on resume,
  // bfcache restore and `online`. Only a 200 whose body names a UUID counts; a
  // 404, a network error or junk is "unknown". A redirect is an expired session
  // and is announced as `oxy:session-expired`, never read as a new build. A tab
  // still behind re-reports (`stale`) every 6 h, so the operator's 24 h
  // open-builds window keeps counting it.
  //
  // ANNOUNCE: a cancelable `oxy:update-available` {from, to}, once per target.
  // Cancelling it means the app takes over (`__oxyUpdate.apply()` when ready).
  // `oxy:update-status` fires whenever `status()` changes.
  //
  // APPLY (auto): no sooner than 60 s after detection — the per-pod resolution
  // cache TTL, so every pod serves the new build by then — and only at a safe
  // moment: a path-changing push/pop navigation, 2 min without input and no
  // editable element focused, or coming back after 10+ min hidden. Never while
  // the app holds a `defer()` token, never offline.
  //
  // STALE CHUNK (observe and auto): see `recoverChunk`.
  //
  // LOOP GUARD, per app (apps on one origin share sessionStorage): a reload
  // records its target first. If the reloaded document is still the old build
  // (a stale pod, or the worker's cached-shell fallback), wait 90 s and try once
  // more, then stop and report `guarded`. Independently, a tab gets at most two
  // automatic reloads per 5 min whatever the target — two pods disagreeing
  // mid-publish cannot bounce it between builds. No storage, no reload.
  //
  // Deliberately NOT here: `skipWaiting` (it swaps the worker under every other
  // open tab — see sw.js), a `window.fetch` wrapper, or any push channel.
  var mode = /^(off|auto|prompt)$/.test(app.updates) ? app.updates : "observe";
  var UUID_RE = /^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$/;
  var CHUNK_RE =
    /Failed to fetch dynamically imported module|Importing a module script failed|error loading dynamically imported module|Unable to preload CSS/i;
  var MIN = 60000;
  var current = String(app.buildId || "").toLowerCase();
  var GUARD_KEY = "oxy-update:" + (app.appId || base);
  var loadedAt = Date.now();
  var seen = {};
  var pending = null;
  var expired = false;
  var holds = {};
  var holdSeq = 0;
  var lastInput = loadedAt;
  var hiddenAt = 0;
  var probing = null;
  var reloading = false;
  var asked = null;
  var stayed = {};
  var capReported = false;
  var notBefore = 0;
  var updPath = location.pathname;

  function readGuard() {
    try {
      var g = JSON.parse(sessionStorage.getItem(GUARD_KEY) || "null");
      return g && typeof g === "object" ? g : null;
    } catch (e) {
      return null;
    }
  }
  function writeGuard(value) {
    try {
      sessionStorage.setItem(GUARD_KEY, JSON.stringify(value));
      return true;
    } catch (e) {
      return false;
    }
  }

  function report(phase, how, from, to, staleMs) {
    push("oxy-update", {
      phase: phase,
      how: how,
      from: from,
      to: to,
      stale_ms: Math.max(0, Math.round(staleMs || 0)),
      build: current
    });
  }

  function setStatus(p, e) {
    if (p === pending && e === expired) return;
    pending = p;
    expired = e;
    window.dispatchEvent(new CustomEvent("oxy:update-status"));
  }

  function probe(how) {
    if (probing) return probing;
    probing = fetch(base + "__oxy/build", {
      cache: "no-store",
      credentials: "same-origin",
      redirect: "manual"
    })
      .then(function (r) {
        if (r.type === "opaqueredirect") {
          if (!expired) {
            setStatus(pending, true);
            window.dispatchEvent(new CustomEvent("oxy:session-expired"));
          }
          return null;
        }
        return r.status === 200 ? r.json() : null;
      })
      .then(function (body) {
        var id = body && typeof body.buildId === "string" ? body.buildId.toLowerCase() : "";
        if (!UUID_RE.test(id)) return null;
        found(id, how);
        return id;
      })
      .catch(function () {
        return null;
      })
      .then(function (id) {
        probing = null;
        return id;
      });
    return probing;
  }

  function found(id, how) {
    var now = Date.now();
    // Rolled back to the build we run: nothing is pending any more.
    if (id === current) return setStatus(null, false);
    var t = seen[id];
    setStatus(t || (seen[id] = { to: id, at: now, beat: now }), false);
    if (t) {
      if (now - t.beat >= 6 * 60 * MIN) {
        t.beat = now;
        report("stale", how, current, id, now - t.at);
      }
      return;
    }
    report("detected", how, current, id, 0);
    var ev = new CustomEvent("oxy:update-available", {
      cancelable: true,
      detail: { from: current, to: id }
    });
    if (!window.dispatchEvent(ev)) pending.cancelled = true;
  }

  function held() {
    var now = Date.now();
    for (var k in holds) if (now - holds[k] < 12 * 60 * MIN) return true;
    return false;
  }

  function reloadTo(to, how) {
    if (reloading) return true;
    var g = readGuard() || {};
    var now = Date.now();
    var log = (g.log || []).filter(function (t) {
      return now - t < 5 * MIN;
    });
    var again = g.to === to;
    if (again && g.n - (stayed[to] || 0) >= 2) return false;
    if (log.length >= 2) {
      if (!capReported) report("guarded", how, current, to, now - log[0]);
      capReported = true;
      return false;
    }
    log.push(now);
    var at = again ? g.at : (seen[to] && seen[to].at) || now;
    var n = again ? g.n + 1 : 1;
    if (!writeGuard({ to: to, from: current, how: how, n: n, at: at, log: log })) return false;
    reloading = true;
    // Added after the app's own listeners, so it sees whether the app asked
    // "leave this page?" (see `refused`).
    window.addEventListener(
      "beforeunload",
      function (e) {
        var rv = e.returnValue;
        if (e.defaultPrevented || (typeof rv === "string" && rv)) asked = to;
      },
      { once: true }
    );
    location.reload();
    return true;
  }

  // A viewer who chose to stay at the app's "leave this page?" prompt left this
  // page running with `reloading` set, so `apply()` and the Update ready pill
  // did nothing for the rest of its life. Their next input on the page is the
  // sign they stayed. It counts as the app cancelling `oxy:update-available`:
  // nothing reloads onto that build by itself again, but `apply()` still does.
  //
  // The same input can also come from a viewer who picked Leave and tapped the
  // old page before the reload committed, so the loop guard is left as written:
  // the new page still finds its target (and reports `applied`), and the try
  // still spends the 5 min budget. Only this page, in memory, excuses it from
  // the per-target count (`stayed`), so two refusals do not end `apply()` for
  // that build. A page that really reloads starts with none excused.
  function refused() {
    if (!asked) return;
    stayed[asked] = (stayed[asked] || 0) + 1;
    if (seen[asked]) seen[asked].cancelled = true;
    asked = null;
    reloading = false;
  }

  function maybeApply(how) {
    if (!pending || pending.cancelled || held() || navigator.onLine === false) return false;
    var age = Date.now() - pending.at;
    var auto = mode === "auto" || (mode === "prompt" && age >= 24 * 60 * MIN);
    if (!auto || age < MIN || Date.now() < notBefore) return false;
    return reloadTo(pending.to, how);
  }

  function editing() {
    var el = document.activeElement;
    var tag = el && el.tagName;
    return (
      tag === "INPUT" || tag === "TEXTAREA" || tag === "SELECT" || !!(el && el.isContentEditable)
    );
  }

  // A lazy chunk the page never fetched is gone once a publish replaces it, and
  // the screen that needed it is broken — but the rest of the page may still
  // hold unsaved work in memory, which a reload discards. So the reload waits
  // until 60 s after load (before that, pods may still disagree about the live
  // build, and a reload could put a fresh page on the OLD one), and is skipped
  // while the app holds a defer token, an editable element is focused, the app
  // cancelled `oxy:update-available` for that build, or it cancels the
  // `oxy:chunk-stale` {from, to} dispatched just before. Skipped means the
  // error is reported as before.
  //
  // The report waits on that decision, up to the floor. A page hidden or
  // unloaded first (a viewer reloading or closing the broken screen) reports
  // at once (`settleChunks`), because `finalize` sends only what was noted.
  // Hidden is settled too, not only `pagehide`: on mobile it is the last
  // signal a closed tab reliably gives. The cost is that a tab hidden and
  // shown again inside the floor has already reported the error, and the
  // decision may still reload it. The error is then counted although the
  // reload recovered it. That over-count is accepted, because the
  // alternative is losing the error.
  var undecided = [];
  function settleChunks() {
    var due = undecided;
    undecided = [];
    for (var i = 0; i < due.length; i++) due[i]();
  }
  function chunkReload(to) {
    if (held() || editing() || (seen[to] && seen[to].cancelled)) return false;
    var ev = new CustomEvent("oxy:chunk-stale", {
      cancelable: true,
      detail: { from: current, to: to }
    });
    return window.dispatchEvent(ev) && reloadTo(to, "chunk");
  }
  function recoverChunk(fallback) {
    var reported = false;
    var noteOnce = function () {
      if (!reported) fallback();
      reported = true;
    };
    undecided.push(noteOnce);
    setTimeout(function () {
      probe("chunk").then(function (id) {
        undecided = undecided.filter(function (f) {
          return f !== noteOnce;
        });
        if (!(id && id !== current && chunkReload(id))) noteOnce();
      });
    }, Math.max(0, loadedAt + MIN - Date.now()));
  }

  if (mode !== "off" && UUID_RE.test(current) && typeof fetch === "function") {
    var guard = readGuard();
    if (guard && guard.to === current) {
      report("applied", guard.how, guard.from, guard.to, Date.now() - guard.at);
      // The per-target count is done; the 5 min budget (`log`) is not.
      writeGuard({ log: guard.log });
    } else if (guard && guard.to && guard.n >= 2) {
      if (!guard.reported) report("guarded", guard.how, current, guard.to, Date.now() - guard.at);
      guard.reported = 1;
      writeGuard(guard);
    } else if (guard && guard.to) {
      // The reload landed on the old build. Ask again now, and allow the one
      // retry no sooner than 90 s from here.
      notBefore = Date.now() + 90000;
      probe("poll");
    }

    onRoute = function (mayApply) {
      var changed = location.pathname !== updPath;
      updPath = location.pathname;
      if (changed && mayApply) maybeApply("route");
    };
    onChunk = function (r, fallback) {
      var hit = !!r && (r.name === "ChunkLoadError" || CHUNK_RE.test(safeString(r.message)));
      if (hit) recoverChunk(fallback);
      return hit;
    };

    window.__oxyUpdate = {
      defer: function () {
        var id = ++holdSeq;
        holds[id] = Date.now();
        return function () {
          delete holds[id];
        };
      },
      apply: function () {
        return !!pending && reloadTo(pending.to, "app");
      },
      check: function () {
        return probe("app");
      },
      status: function () {
        return {
          mode: mode,
          current: current,
          available: pending ? pending.to : null,
          sessionExpired: expired
        };
      }
    };

    var schedulePoll = function () {
      setTimeout(function () {
        if (document.visibilityState !== "hidden") probe("poll");
        schedulePoll();
      }, 5 * MIN * (0.8 + Math.random() * 0.4));
    };
    schedulePoll();

    var resume = function () {
      var away = hiddenAt ? Date.now() - hiddenAt : 0;
      hiddenAt = 0;
      // Coming back is activity: the idle clock restarts, so a short lock is
      // never read as two idle minutes the moment the screen wakes.
      lastInput = Date.now();
      probe("resume").then(function () {
        if (away >= 10 * MIN) maybeApply("resume");
      });
    };
    // Both registered before `finalize`'s own listeners below, so a settled
    // chunk error is noted before the errors are sent.
    document.addEventListener("visibilitychange", function () {
      if (document.visibilityState !== "hidden") return resume();
      hiddenAt = hiddenAt || Date.now();
      settleChunks();
    });
    window.addEventListener("pagehide", settleChunks);
    window.addEventListener("pageshow", function (e) {
      if (e.persisted) resume();
    });
    window.addEventListener("online", function () {
      probe("poll");
    });
    window.addEventListener("vite:preloadError", function (e) {
      // `preventDefault` must be synchronous, so it is used only when the new
      // build is already known and the 60 s floor has passed. Otherwise Vite's
      // error proceeds as it would have and the probe decides; an app that
      // catches the rejection itself (an error boundary) still gets the reload.
      if (!pending || Date.now() - loadedAt < MIN) recoverChunk(function () {});
      else if (chunkReload(pending.to)) e.preventDefault();
    });

    // Every mode: `apply()` and chunk recovery reload in observe too.
    ["pointerdown", "keydown", "touchstart"].forEach(function (type) {
      window.addEventListener(
        type,
        function () {
          lastInput = Date.now();
          refused();
        },
        { capture: true, passive: true }
      );
    });
    if (mode === "auto" || mode === "prompt") {
      // Idle means a VISIBLE page nobody is touching. A hidden tab is left to
      // the resume path, which knows how long it was away.
      setInterval(function () {
        var idle = Date.now() - lastInput >= 2 * MIN;
        if (pending && idle && document.visibilityState !== "hidden" && !editing()) {
          maybeApply("idle");
        }
      }, 15000);
    }
  }
  // @oxy-updates:end

  // ── lifecycle ─────────────────────────────────────────────────────────────
  // `visibilitychange → hidden` is the only reliable end-of-session signal on
  // mobile; `pagehide` covers desktop navigations that skip it — and an
  // ordinary desktop navigation fires BOTH, in that order. Every emitter below
  // therefore has to be idempotent on its own: `emitVitals` via `vitalsSent`,
  // `emitErrors` by emptying its map, `emitEngagement` by advancing
  // `pageStart`. Draining the queue does not provide that — a flush sends, and
  // sending is exactly what makes a second push a second row.
  function finalize() {
    emitVitals();
    emitErrors();
    emitEngagement("hidden");
    flush(true);
  }
  document.addEventListener("visibilitychange", function () {
    if (document.visibilityState === "hidden") {
      finalize();
      return;
    }
    // Coming back starts a fresh engagement window. Without this, tabbing away
    // and back twice reports the second stretch as everything since the page
    // loaded — so a dashboard left open in a background tab all day reports
    // hours of "engagement" nobody spent looking at it.
    pageStart = Date.now();
  });
  window.addEventListener("pagehide", finalize);

  // A first flush shortly after load carries the vitals for the initial paint
  // even if the visitor never leaves the tab (a dashboard left open all day).
  setTimeout(function () {
    emitVitals();
    flush(false);
  }, 10000);
})();
