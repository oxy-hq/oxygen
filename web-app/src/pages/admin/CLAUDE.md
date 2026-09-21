# Admin panel conventions

Everything under `src/pages/admin/` is an operator surface: dense, scannable, read
by people who keep many rows on screen at once. It is deliberately smaller and
tighter than the customer-facing product.

## The kit — build a page out of these, not out of divs

Four primitives carry every page. A new page that needs a fifth is a signal to
extend the kit, not to hand-roll beside it. They live in `components/`.

| Use | For | Replaces |
| --- | --- | -------- |
| `AdminPage` | the page frame: width, padding, rhythm, `<h1>`, description, actions | the `mx-auto max-w-… p-6 …` wrapper and its header |
| `AdminAsync` | loading / failed / empty around any fetch | `isLoading ? <Skeleton/> : isError ? <div>Failed to load X.</div> : …` |
| `ADMIN_TONE` (`adminTone.ts`) | any colour that means a status | `text-amber-700 dark:text-amber-400` and friends |
| `adminNav.ts` | the route map: labels, groups, who may reach what | a second list of routes anywhere |

```tsx
<AdminPage width='wide' description='What this surface is for.' data-testid='admin-compiles'>
  <AdminAsync query={compiles} noun='compiles' rows={5}
    isEmpty={(d) => d.rows.length === 0}
    empty={<AdminEmptyState icon={FileCheck} title='Nothing compiled yet.' />}>
    {(data) => <CompilesTable rows={data.rows} />}
  </AdminAsync>
</AdminPage>
```

**Do not pass `title` to `AdminPage`.** The heading comes from `adminNav.ts`, the
same source the rail label and the topbar breadcrumb read, so the three cannot
disagree — they did, for a long time ("Compile revisions" in the rail, "Compiles"
on the page, a third spelling in the topbar's own table). Pass `title` only on a
page that is *about* an entity the route can't name (an org, a user, a workspace).

**Width is a role, not a measurement:** `narrow` (a form) · `default` (prose and a
card) · `wide` (a table someone scans) · `full` (a split pane). Nineteen pages once
had twelve different spellings of the same wrapper.

**Pass the whole query object**, never `const { data = [] } = useThing()`. That
default makes a failed fetch render as an empty state — a server that is down and a
server with nothing to say look identical. `AdminAsync` tells them apart, shows the
server's own message, and offers a Retry; the fifteen hand-written error blocks it
replaced offered none.

**The same rule one layer up: a source that did not answer is a third outcome**, not an
empty one. `AdminHome`'s reports shipped taking `data | undefined` and falling through to
their "all clear" sentence, so a 500 on workspace health rendered under a green tick as
"No workspace is failing its health checks" — on the page whose whole job is to answer
*does anything need me?*. They now take the query and return `okTone: "unknown"`, which
the page renders with a warning glyph and "Couldn't check …". The test that was supposed
to catch this asserted the sentence didn't match `/All \d/`, which the base sentence
satisfies while still claiming health — **it passed for the wrong reason**, which is why
the rule below about mutating before trusting is not optional.

**Never invent a status colour.** `ADMIN_TONE[tone]` gives `{ dot, text, bg, ring }`
for `ok | info | warn | danger | muted`, already correct in both themes — which is
why none of them carry a `dark:` variant. A new resource status maps onto an
existing tone; it does not get a sixth. On this surface colour means *something is
wrong*, so `info` stays in the foreground colour rather than borrowing a blue.

**`RoleBadge` is the one sanctioned exception**, and the only other place a colour
may be chosen. It encodes *identity* — where authority comes from (staff amber /
org neutral / partner brand) — which is a different axis from *status*, and mapping
it onto the tones would have the users list announce that every Global Owner is a
warning. Its light/dark pairs are spelled out because the raw `--warning` and
`--info` tokens are display colours: neither clears 4.5:1 as small text on white.
Anything that is not a status and not a role badge has no business being coloured.

**A capability gate reads the map.** `itemReachable(item, standing)` is the one rule
for "may this principal see this". The console home and the ⌘K palette both filter
through it, so neither can offer a room the server will 403.

## Type scale (HARD — no exceptions)

| Role | Class |
| ---- | ----- |
| Page title (`h1`) | `text-xl font-semibold tracking-tight` (via `AdminPage`) |
| Card / section heading (`h3`) | `text-sm font-semibold` |
| Collapsible section label | `text-[10px] uppercase tracking-[0.16em]` |
| **Body, table cells, empty states, help text** | `text-xs` |
| Metric value | `text-sm tabular-nums` (hero metric: `text-2xl`) |

**`text-xs` is the default.** Reach for anything larger only from the table above.
`text-base` and `text-lg` do not appear in the admin panel at all — if a size
feels too small, the fix is weight or color (`font-medium`, `text-foreground` vs
`text-muted-foreground`), not points.

**One exception, and only one:** a monogram glyph sized to its avatar box
(`OrgLogo`'s `lg: "size-16 text-2xl"`). That is artwork filling a fixed square,
not type — leave it alone, and don't let a scale sweep "fix" it.

Icons follow the text: `size-3` beside `text-xs`, `size-3.5` in a heading row.

## Long text belongs to the reader

A failure message is the most important text on the page it appears on, and it is
the one most often truncated. Wrap it (`whitespace-pre-wrap break-words`, mono) and
give it a column of its own. Workspace health used to print a 206-character
connector error into a table cell competing with three other columns, so it ran off
the right edge with no way to read the end of it — nine times over, once per
affected workspace, for a single underlying cause.

**Group by cause, not by row.** A platform breaks in platform-shaped ways: one
refused connection lands on every workspace that touches it. `groupByCause` in
`AdminWorkspaceHealth/` is the worked example — state the failure once, list what it
took down beneath it.

## Naming & targetability

An operator pointing at a bug should be able to name the component from the DOM.

- Every section, list, row, empty state, and stat carries a `data-testid` of the
  form **`admin-<area>-<element>`**, kebab-case
  (`admin-app-activity-visitors-empty`, `admin-app-dossier-section-functions`).
- **Key the testid off a stable id, not display copy.** `DossierSection` takes an
  `id: SectionId` for exactly this reason — the title is editable prose, the id
  is not.
- Sub-components get their own named file once a file passes ~150 lines. Four
  anonymous inner components in one 215-line file is what made `Activity`
  untargetable; it is now `Activity/components/Activity{Summary,Visitors,Events,Stat}.tsx`.
- Name a component for what it *is* on screen (`ActivityVisitors`), not for its
  position in a layout (`Section2`).

## Tests

The pure parts are unit-tested and the tests are the specification: `adminNav.test.ts`
(who reaches what, and what each page is called), `AdminAsync.test.tsx` (which of the
four states wins), `groupByCause.test.ts` (nine identical failures are one incident),
`AdminHome/findings.test.ts` (what is worth waking someone for — and what is not:
an idle worker on a quiet queue, an app with no traffic).

Run them with `./node_modules/.bin/vitest run src/pages/admin`. When you change one
of these rules, **mutate the source and watch the test fail before you trust it** —
two of the bugs these files exist for were invisible to an assertion that passed for
the wrong reason.
