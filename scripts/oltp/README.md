# Per-org OLTP Postgres

One Postgres **per org**. One schema **per writer** inside it — `app_<slug>` for
a custom app, `raw_<source>` for an Airway pipeline — plus `oxy_meta` for Oxy's
own ledger. Writers cannot see each other. One read-only role, `oxy_analyst_ro`,
reads whatever has been published to it, and is what every human and agent query
resolves to.

```
oxy_org_<org-uuid>
├── app_bookings   ← app_bookings_rw    (the custom app writes here, and nowhere else)
├── raw_toast      ← raw_toast_rw       (the Airway pipeline, likewise)
└── oxy_meta       ← owner only         (migration ledger; the analyst cannot read it)
                     oxy_analyst_ro     SELECT on what is published, nothing else
```

Every boundary above is a Postgres privilege, not an application check. The
proof lives in `crates/oltp/tests/integration/grants.rs`, which asserts each
denial against a real Postgres.

- **Operations** — provider landmines, what each failure message means,
  deprovisioning, the Airway limits:
  [`internal-docs/per-org-oltp-postgres.md`](../../internal-docs/per-org-oltp-postgres.md)
- **Design** — why it is shaped this way: the "Why it is shaped this way"
  section of the same file (design record in git history: PR #2851)

---

## Set it up by hand

There is no demo script: the verbs are `oxyc oltp` (over the running server's
admin routes, so they need `oxyc login` or `OXY_TOKEN` and `--env`/`--target`)
plus the Rust `oxy oltp apply | audit | rotate`, which still read
`OXY_DATABASE_URL` directly. `--org` takes a slug or UUID for `oxyc`, and a UUID
or a member's email for `oxy`. Start the stack the normal way
(`oxy start --enterprise`) and point at the local provider:

```bash
export OXY_OLTP_PROVIDER=local
export OXY_OLTP_ADMIN_URL="$OXY_DATABASE_URL"   # neon: OXY_OLTP_NEON_API_KEY + _ORG_ID instead
```

The provider vars are all of it. Per-org OLTP is always on — there is no
runtime flag — so once a provider is configured the server side (the console
Provision button, `postgres_managed` queries) and the CLI verbs below both
work.

```bash
# 1. a database, its writers and the analyst credential (idempotent)
oxyc oltp provision --target http://localhost:3000 --org <org-slug> --writer app:bookings --writer pipeline:toast

# 2. compile the workspace, then apply the schemas it carries
oxy compile --workspace-path "$PWD/examples" --workspace-id <ws> --skip-migrations
oxy oltp apply --org you@oxy.tech

# 3. `raw_*` is analyst-readable on creation; `app_*` is opt-in
oxyc oltp expose --target http://localhost:3000 --org <org-slug> --writer app:bookings

# 4. look around
oxyc oltp status  --target http://localhost:3000 --org <org-slug>
oxyc oltp audit   --target http://localhost:3000 --org <org-slug>   # every role's real authority, read from pg_roles; exit 9 on a finding
oxyc oltp connect --target http://localhost:3000 --org <org-slug>   # psql as the read-only analyst
```

Or from the admin console: **Admin → OLTP databases** provisions, exposes and
deprovisions without a terminal.

## Seed data

`examples/` carries enough to exercise the whole path:

| Path | What it is |
| --- | --- |
| `examples/schemas/000*.sql` | The migrations `oxy oltp apply` runs — bookings tables, a Toast raw table, and demo orders. |
| `examples/oltp_semantics/views/*.view.yml` | Semantic views over `app_bookings` and `raw_toast`, so the agent can be asked questions. |
| `examples/oltp.agentic.yml` | An analytics agent scoped to those views only — no warehouse credentials needed. |
| `examples/pipelines/oltp_toast.airway.yml` | An Airway pipeline whose destination is `database: oltp`, i.e. its own writer. |

The pipeline reads `public.demo_pos_sales`, which nothing creates for you:

```sql
CREATE TABLE public.demo_pos_sales (
  ticket_id text, location text, net_sales text, business_date text, voided boolean
);
INSERT INTO public.demo_pos_sales VALUES
  ('1','Harbor',   '655.40', (CURRENT_DATE - 1)::text, false),
  ('2','Downtown','1240.50', (CURRENT_DATE - 2)::text, false),
  ('3','Airport',  '980.25', (CURRENT_DATE - 4)::text, false),
  ('4','Downtown','1517.00', (CURRENT_DATE - 5)::text, false),
  ('5','Harbor',   '742.75', (CURRENT_DATE - 6)::text, false),
  ('6','Airport', '1103.00', (CURRENT_DATE - 8)::text, false);
```

Text columns because that is what airway's Postgres source can land — it
converts only `String`/`i64`/`f64`/`bool` and nulls the rest, so a `numeric`
column arrives NULL and the load fails. The view casts them back.

Dates are relative so "last week" means something whenever you run it, and they
straddle both readings of it (previous calendar week *and* trailing seven days),
because the agent picks one per run and does not pick the same one twice.

Then: `oxyc airway run pipelines/oltp_toast.airway.yml --workspace <ws> --target http://localhost:3000`
(the server runs the pipeline it has compiled from `examples/`; `oxy airway run`
was deleted for it on 2026-11-21), and ask the agent `what were net sales by
location last week?`.

## If startup fails on a missing migration

The five OLTP migrations were squashed into one, so a database that ran the
earlier build has applied versions whose files no longer exist — and SeaORM
refuses to start on that. It reads as a broken build; it is a ledger that needs
one delete. Anything that ran this branch before the squash is affected — a
review app, a shared staging box, a colleague's laptop — while fresh databases
and CI are not. The tables are already in the right shape:

```sql
DELETE FROM seaql_migrations_oltp
 WHERE version <> 'm20260804_000001_create_oltp_tables';
```

## Housekeeping

| Command | Does |
| --- | --- |
| `just oltp-status` | Is the Postgres container up. |
| `just oltp-psql analyst\|app\|pipeline\|owner` | psql as one of the provisioned roles. |
| `just oltp-stop` | Free `:3000`/`:5173`, stop the containers. Deletes nothing. |
| `just oltp-down` | Drop tenant rows, roles and every `oxy_org_*` database. |
| `just oltp-clean` | Both of the above, in that order. |

`oltp-down` has no `WHERE`: it clears OLTP state for **every org on the
cluster**, and it does not delete Neon projects — for a Neon tenant that
combination destroys Oxy's only record of a project that keeps billing, so
delete those in the console first.
