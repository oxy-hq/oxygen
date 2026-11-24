# {{APP_DISPLAY_NAME}}

An Oxy **custom app** with a server-side **Oxy Function** — a Vite + React
bundle that Oxy serves inside your workspace, plus a handler that runs on Oxy's
managed runtime with data-plane access. Scaffolded by `create-oxy-app`.

## Quick start

```bash
pnpm install
pnpm dev          # http://localhost:5173 — proxies /api → your local oxy server
pnpm email:dev    # render emails/Welcome.tsx in a browser (no server needed)
```

## Layout

| Path | What it is |
| --- | --- |
| `oxy-app.json` | The app's identity, launcher-card metadata, and the `functions` block that declares each handler and its capabilities. |
| `src/App.tsx` | Your UI, and the three surfaces (`useQuery`, `useSemanticQuery`, `useFunction`). |
| `functions/notify.ts` | A server-side handler. Bundled and shipped by `oxyc publish`; runs on Oxy's isolate. |
| `emails/Welcome.tsx` | A **preact** email template rendered to HTML by `@oxy-hq/sdk/email`. |
| `src/index.css` | The app's own CSS, after the SDK's. The palette and light/dark come from `@oxy-hq/sdk/theme.css`; restate no token here. |
| `tests/design-system.test.ts` | `pnpm test`: the SDK's theme guard over the CSS (`checkThemeCss()`), and its no-copy check over `src/` (`checkSdkUiCopies()`), both from `@oxy-hq/sdk/testing`. |
| `public/` | Static assets copied to the bundle root — `icon.svg` and `card.svg`. |

## The design system

The look is Oxygen's, and the app stays on it by importing, never copying:

- **Stylesheets**, in `src/main.tsx`, in this order: `@oxy-hq/sdk/theme.css`
  (every token, light and dark), `@oxy-hq/sdk/shell.css`, `@oxy-hq/sdk/ui.css`,
  then `src/index.css`. `src/index.css` imports `@oxy-hq/sdk/tailwind.css`
  **before** `tailwindcss`, so `bg-card` or `text-muted-foreground` resolve to the
  theme's tokens.
- **Pieces** come from `@oxy-hq/sdk/ui`: `Panel`, `Stat`, `DefaultTabs`,
  `PanelTable`, `LoadError`, `EmptyState`, `SidePanel`, `Modal`, `Button`,
  `Field` and the rest.
- **`pnpm test` holds it**, with two checks from `@oxy-hq/sdk/testing`.
  `checkThemeCss()` fails a stylesheet that restates a theme token or decides
  light/dark itself, and `checkSdkUiCopies()` fails when `src/` defines a
  component under the name of an SDK piece (a local `SidePanel`, `LoadError`,
  `DefaultTabs`), naming the import to use instead. A local variant you keep on
  purpose says why, in its file:

  ```tsx
  // oxy-ui-local: SidePanel -- docks left as well as right, which the SDK's does not
  ```

## Oxy Functions

A function is a TypeScript handler that runs **server-side**, so it can reach
things the browser must not: the warehouse, secrets, object storage, email, and
external APIs. Declare it in `oxy-app.json` and invoke it with `useFunction`:

```tsx
const { invoke, data, error, isLoading } = useFunction("notify");
await invoke({ name: "Ada" });
```

Capabilities are **fail-closed** — a handler can only do what its manifest entry
grants. `notify` declares `"email": { "send": true }`; without it the host
rejects the send. The same applies to `secrets`, `storage`, and the rest.

A function can also run as a **cron job** (add a `schedule` to its manifest
entry) rather than being called from the UI.

### Sending email safely

The platform owns the `from` address — mail goes out from a shared verified
sender. That makes a caller-supplied recipient an open relay, so **always derive
recipients server-side**: `notify` sends to `ctx.user.email` (the invoking user),
never to an address taken from the request body. Template *data* can come from
the request; the recipient must not.

One catch if you also give this function a `schedule`: a background run has no
caller to attribute it to — a cron tick, or an operator's manual *Run now* from
the admin console — and `ctx.user.email` is then the synthetic
`schedule+notify@system.oxy`
— a dead address. Branch on `ctx.user.kind === "system"` (never on the email
string) and look the real recipients up from your own data on that path. `kind`
needs an Oxy server from 2026-08-21 or later; against an older one, mark the
schedule's configured `input` instead of inferring.

Email templates are **preact** components, not React — preact runs inside the
Functions isolate and react-dom does not. `@oxy-hq/sdk/email`'s
`render(Component, props)` turns one into HTML.
Give a prop its default with a default parameter (`{ name = "friend" }`):
preact 11 ignores `defaultProps` on a function component, without an error.

Locally, set `OXY_APP_EMAIL_LOCAL_TEST=1` on your dev server to preview the
rendered email in the browser instead of sending it through SES.

**Attachments** — `notify` attaches a small generated CSV. Each attachment says
how to read its `content`, and the right `encoding` follows from where the bytes
came from:

| Source | Use |
| ------ | --- |
| Text you just generated (CSV/JSON/HTML) | `encoding: "utf8"` |
| A file in the app's asset store | `ctx.storage.get(key, { encoding: "base64" })` |
| A remote file | `ctx.fetch(url, { encoding: "base64" })` |
| Bytes you built (`Uint8Array`) | `bytesToBase64(bytes)` from `@oxy-hq/sdk` |

Prefer `"utf8"` for anything textual: `btoa` reads strings as Latin1, so
`btoa(csvWithAccents)` yields mojibake rather than an error. Limits are 20
attachments and 10 MiB decoded per send — for larger files, store them with
`ctx.storage` and email a presigned link instead.

## Querying your data

```tsx
const { rows, loading, error } = useQuery({ sql: "SELECT * FROM orders LIMIT 10" });

const { rows: semantic } = useSemanticQuery({
  topic: "store_performance",
  dimensions: ["store_performance.store_id"],
  measures: ["store_performance.total_sales"],
  limit: 5
});
```

Auth rides the session cookie — Oxy only serves this bundle after a membership
check, so there is no token to manage in the frontend.

## Launcher card

The HQ home shows each app as a card: `icon` is the small mark, `art` is the
wide preview. Both are **relative** paths in `oxy-app.json`, served from the
bundle root — never hardcode the base path. Replace `public/icon.svg` and
`public/card.svg` with your own.

## Deploying

```bash
npm install -g @oxy-hq/cli       # once; installs oxyc
oxyc login   --env production    # once per env; caches a token
oxyc publish --env production    # build + ship to the draft channel
oxyc publish --env production --promote   # …straight to live
```

`oxyc publish` bundles the frontend **and** `functions/*`, resolves the target
org + project, and uploads it. No CI required and no project id in git.
