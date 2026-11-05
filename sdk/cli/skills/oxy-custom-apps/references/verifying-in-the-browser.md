# Verify it in the browser (CI does not check a published bundle)

Nothing checks a bundle before it reaches users. `tsc` and `vite build` pass on
a card rendering a red error banner, a grid with ragged rows, a button wired to
nothing, or a component misreading its function's response shape. **The build is
not the check — driving the published app is.**

## An agent on a sandbox agent token: through `oxyc proxy`

The token signs no browser in, so a browser tool cannot open the sandbox's
own host (the `url` from `env create`). Put the proxy in between instead. It
holds the token, adds `X-Oxy-App-Env`, and serves your sandbox's page on
localhost:

```bash
oxyc proxy --app <org>/<app> --app-env dev-x --port 3000 --env <deployment> --token-env OXY_TOKEN   # background
```

Then drive `http://127.0.0.1:3000/customer-apps/<org>/<app>/` as below. For a
plain "did it mount" assertion, `oxyc checks run … --browser` needs no proxy.
Cards fed by the deployment's `/api` answer `404` to the token, so they render
without data; say which ones in your report
([Check its page](sandbox-agent-loop.md#check-its-page)). A person on their own
login opens the sandbox's `url` directly.

## Drive the page

Use a browser-automation MCP server — the Playwright one (`@playwright/mcp`)
fits. After `oxyc publish`, for anything touching `src/**` or a function a
card calls:

```
browser_navigate  → the published /customer-apps/<org>/<slug>/ URL (the proxy's, for an agent)
browser_find      → locate a control by its text, get its ref
browser_click     → drive it
browser_evaluate  → MEASURE (grid widths, computed styles, element counts)
browser_take_screenshot → look, and compare before/after for visual work
```

**Calling the function endpoint is not verifying the UI.** A bearer-token
`fetch` proves the handler returns JSON. It does not prove the card renders it,
that the button is wired, or that a timer stops when told — and it can mislead
outright: a bearer-token call and a browser session don't always resolve the
same app state, so such a check can "pass" against data the page never shows.
If the claim is about what a person sees, drive the page.

`browser_evaluate` is the underused one. Design questions are usually
measurable, and measuring beats squinting:

```js
// Ragged rows? Sum each grid row's children against the container.
const grid = document.querySelector("main > div");
const rows = {};
for (const el of grid.children) {
  const b = el.getBoundingClientRect();
  (rows[Math.round(b.top)] ||= []).push(Math.round(b.width));
}
// any row summing well under the container width is a gap
```

## Two prerequisites that bite

- **The app is behind auth.** The browser must hold a session for the target
  Oxy deployment — sign in there first — or go through `oxyc proxy`, which
  carries the credential for it.
- **`pnpm dev` against cloud data returns `origin not allowed`** — the data API
  rejects a localhost origin for a published app. Drive the published URL, or
  run the `oxyc proxy` sidecar
  ([reading-data.md](reading-data.md#working-locally-against-cloud-data-oxyc-proxy)).

## Check the whole page

Not just the card you changed. A shared component (a layout wrapper, a design
primitive) touches every card, and a section you didn't open is where the
regression hides.
