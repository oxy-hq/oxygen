# {{APP_DISPLAY_NAME}}

An Oxy **custom app** — a Vite + React bundle that Oxy serves inside your
workspace and that queries your project through `@oxy-hq/sdk`. Scaffolded by
`create-oxy-app`.

## Quick start

```bash
pnpm install
pnpm dev        # http://localhost:5173 — proxies /api → your local oxy server
```

It renders on first run with nothing else set up: the starter query is a
literal `SELECT`, so you get a working app before you point it at real data.

## Layout

| Path | What it is |
| --- | --- |
| `oxy-app.json` | The app's identity + launcher-card metadata: `slug`, `name`, `icon`, `art`. Lives at the project root (**not** under `public/`). |
| `src/App.tsx` | Your UI, and the two data surfaces (`useQuery`, `useSemanticQuery`). Start here. |
| `src/index.css` | The app's own CSS, after the SDK's. The palette and light/dark come from `@oxy-hq/sdk/theme.css`; restate no token here. |
| `tests/design-system.test.ts` | `pnpm test`: the SDK's theme guard over the CSS (`checkThemeCss()`), and its no-copy check over `src/` (`checkSdkUiCopies()`), both from `@oxy-hq/sdk/testing`. |
| `vite.config.ts` | Wires `oxyApp()`, which validates + copies the manifest, proxies `/api`, and resolves the served base path. |
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

## Querying your data

`useQuery` runs raw SQL against the project's warehouse:

```tsx
const { rows, loading, error } = useQuery({ sql: "SELECT * FROM orders LIMIT 10" });
```

`useSemanticQuery` reads measures + dimensions from a semantic topic, so the
SQL stays in your `.view.yml` / `.topic.yml` files instead of this bundle:

```tsx
const { rows } = useSemanticQuery({
  topic: "store_performance",
  dimensions: ["store_performance.store_id"],
  measures: ["store_performance.total_sales"],
  limit: 5
});
```

Auth rides the session cookie — Oxy only serves this bundle after a membership
check, so there is no token to manage in the frontend.

Parameters are sent with the query and quoted by the server, for the database
that runs it:

```tsx
useQuery({ sql: "SELECT * FROM orders WHERE region = {{ params.region | sqlquote }}" },
         { params: { region } });
```

## Launcher card

The HQ home shows each app as a card: `icon` is the small mark, `art` is the
wide preview. Both are **relative** paths in `oxy-app.json`, served from the
bundle root — never hardcode the base path. Replace `public/icon.svg` and
`public/card.svg` with your own (a real screenshot makes a good `art`).

## Deploying

```bash
npm install -g @oxy-hq/cli       # once; installs oxyc
oxyc login   --env production    # once per env; caches a token
oxyc publish --env production    # build + ship to the draft channel
oxyc publish --env production --promote   # …straight to live
```

`oxyc publish` reads `oxy-app.json`, runs the build into `out/`, resolves the
target org + project, and uploads the bundle. No CI required and no project id
in git.

## Server-side functions

Need to run code on the server — hit an external API, send email, keep a secret
out of the browser? That's an **Oxy Function**. Scaffold a copy of this app with
one already wired up:

```bash
pnpm dlx create-oxy-app my-app --template functions
```
