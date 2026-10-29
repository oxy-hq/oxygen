---
name: oxy-experiments
description: >-
  Use when running an Oxy experiment from a terminal or agent — drafting a
  design, choosing its grain (hour, day, week, month), pricing candidate
  designs, registering, watching a live run and its interim reading, aborting,
  or reading the report — with `oxyc experiments`. Covers the lifecycle order,
  the hash handshake that guards every write, why a pricing refusal or a sealed
  result is the answer rather than an error, why an interim is a hint and not a
  result, and what lives in git versus the experiment record.
---

# Oxy experiments with `oxyc`

An experiment is a **record on the deployment**, not a file in the workspace:
one draft design edited in place, priced, registered (which freezes it and
makes it the run), then watched until it is reported or aborted. What the
design *points at* stays in git and is compiled like any other workspace file:
the lever automations (`lever.on.via`, `lever.off.via`), the measures and views
(`outcome.target`, `design.time_dimension`, guardrail criteria). A design that
names an automation or measure the workspace does not have is refused at
pricing or registration — fix the workspace, not the record. So is a lever
whose automation never reads the units it is sent — `{{ unit }}` for
`lever.per: unit`, `{{ units }}` for `per: wave`, or `{{ members }}` under
`assign_by` — since every flip would then switch the whole fleet. Make the
automation act on the units it is given.

There is no file to import: create the draft from a YAML or JSON body
(`create --file`), or start from an existing design's config (`design <name>`).
`owner` is a workspace member's email (any `@` makes it one, matched whatever
its capitals) or else a team name; it receives guardrail alerts, abort notices
and estimate failures. An owner that matches nobody sends them to the
organisation's owners and admins.

Every verb needs `--workspace <id>` (`oxyc api {org}/workspaces` lists them)
and the deployment (`--env dev|staging|local`, or `--target <url>`). Staff
reach a tenant workspace only inside `oxyc assume start --org <slug> --reason …`;
a 403 that says `assume_role_required` means start one, not that the role is
missing.

## The loop

```bash
oxyc experiments list                                   # what exists, and each status
oxyc experiments design customer_promo > promo.yml      # a starting point: the config as YAML
#   edit promo.yml: `experiment:` is the new name; leave design.waves/seed and
#   outcome.mde_at_proposal empty — pricing writes those. Set design.grain if not daily
oxyc experiments create --file promo.yml
oxyc experiments price promo \
  --candidate '{shape: waves, wave_sizes: [20, 20], first_switch_on: 2026-11-02, spacing_days: 14}' \
  --candidate '{shape: switchback, first_period_on: 2026-11-02, period_days: 7, pairs: 6}' \
  --wait
oxyc experiments use promo <proposal-id>                # pricing's keys onto the draft
oxyc experiments register promo --yes                   # irreversible: freezes the design
oxyc experiments status promo                           # flips, guardrails, compliance, estimate
oxyc experiments interim promo                          # the reading so far — a peek, not the result
oxyc experiments result promo                           # sealed until the run is reported
```

Statuses, in order: `draft` → `registrable` (a priced proposal was adopted) →
`registered` → `live` → `closing` → `reported`, or `aborted` from
`registered`/`live`. `registration_refused` carries the reason in the `note`
column.

## Grain: what one reading is

`design.grain` is `day` (the default, and omitted from the config), `hour`,
`week` or `month`. One reading is a unit's outcome over one period of it, and
**every count in the design is in periods**: `pre_days`, `post_days`,
`washout_days`, `anticipation_days`, a candidate's `spacing_days` and
`period_days` keep their `_days` names but mean hours, weeks or months at those
grains. The answer is stated per unit-period (per store-week), and a period
that has not finished is never read.

- Every date must start a period: a **Monday** at week grain, **the 1st** at
  month grain, any date at day grain (the switch happens as that day begins in
  `design.timezone`), and a **whole local hour** at hour grain, written
  `2026-10-26T14:00` (a bare date means midnight; 14:30 is refused). Pricing
  refuses a first date that is not a period start and names the next one.
- **Hour** needs the outcome's time dimension to be a datetime; on a date
  column it is refused. At hour grain a switchback can alternate within a day,
  and the read-back (`when_on`/`when_off`) measures must also have a time of
  day, or no switch inside a day could be confirmed — pricing warns and names
  the measure. Prefer a period that holds the same hours in both halves of a
  pair (12 or 24); hourly outcomes swing hard across the day.
- **Week** is Monday to Sunday. A warehouse whose weeks start on Sunday
  (BigQuery and MySQL by default) is refused at week grain: use day or month.
- Pick week or month when the measure does not add up over days (weekly
  active customers), when most days are empty, or when dates must line up with
  how the business runs. On a measure that adds up and has data every day the
  grain changes the unit of the answer, not the answer.
- What watches a live run stays in **days** at every grain: `grace_days`,
  guardrails, and the arrival-lag warnings.

## Pricing: read a refusal as the result

`price` takes one to six candidates and prices each against the draft. Each
comes back `done` (an MDE), `refused` (why this design cannot answer the
question) or `failed` (a fault). **A refusal is the platform's most valuable
output** — "all 80 units are placed in waves, none is left as a control", or
"a sign-flip test over 4 pairs cannot reach α = 0.05" — so change the candidate
the way the reason says and price again. Do not retry the same candidate.

- Rank candidates on **`mde/unit-day`** only (per unit-period at other
  grains). A grouped design's own `mde` is per *group*-day, so raw `mde`
  numbers are not comparable across candidates.
- `alpha` on a candidate overrides `outcome.alpha` for it. Above 0.05 the
  report can only ever say *directional*, never *significant*.
- A proposal marked `stale` priced an earlier draft; `use` refuses it. Price again.
- `--wait` polls until every candidate settles (default 600 s, `--timeout`);
  without it, read them later with `oxyc experiments proposals <name>`.

## Writes are guarded by the draft's hash

`edit` sends the hash of the draft it read (or `--base-hash` for the one *you*
read earlier); `register` and `retrospective` send the hash of the draft they
read just before posting. If someone edited the draft in between, the server
answers 409 "the design was changed since you read it" — re-read with
`design`, merge, and try again. Never loop on the 409.

An experiment has **one draft**, edited in place until it is registered;
registering locks it, and a **registered design never changes**. To change
one, `duplicate <name> <new-name>` (the UI's *Duplicate as new draft*) and
edit and price the copy.

## Register vs retrospective

- `register` pre-registers: it freezes the design at the server's clock and the
  driver flips levers on the scheduled dates. Do it before the first switch date.
- `retrospective` is for a design whose dates are **all past**: it analyses
  existing data as the experiment's one run and moves no lever. `status` says
  `retrospective_ready` when it is allowed, or `not yet` with the reason.

A retrospective needs no `lever` and no `mde_at_proposal`. A rollout that was not
randomised is declared with `design.assignment: observed`: its waves are the units
that changed and the dates they changed, and it needs no `seed` and no pricing.
Its report reads **Observational**; never price-and-overwrite the waves to fake a
draw (a hand-edited split is reported as written by hand). A retrospective whose
estimate refused (a unit with no rows, coverage) leaves the design editable: fix
it, then `retrospective` again.

A retrospective leaves out a unit with too little data, or listed with none,
and names it under the report's dropped units; it does not refuse the run. A
unit missing a few days keeps the rest and costs no other unit anything; one
under the coverage floor of a single wave's window is left out of that wave only.
`outcome.denominator` makes the outcome a per-unit share (`Σ target / Σ
denominator`): retrospective only, and pricing refuses it. `outcome.scale:
proportional` reads each value as `ln(1 + y)` and reports a % lift per unit
(`report.proportional`, the headline as a share, 0.12 = +12%): use it when units
differ in size by orders of magnitude. Retrospective only; refused with a
denominator, a driver or a switchback, and on a negative value.

A change made in a few **chosen** units (a promotion on three products, a
campaign in one market) is better read against a synthetic control: on an
observed design set `design.comparison: synthetic_control`, and list in
`design.exclude` any unchanged unit the change may still have reached (sibling
products, neighbouring stores). Each treated unit gets its own weighted mix of
unchanged units that tracked it before the switch. It needs one switch date and
at least 10 donors, and is refused with a switchback, strata, a denominator,
carryover or hour grain. `report.synthetic` lists each unit's fit (`pre_fit`: 0
tracks perfectly, 1 is no better than a flat line), its donor weights and its
series; the report is labelled **Not randomised — synthetic control** and
offers no projection.

A one-date design is tested with Welch by default. For a handful of treated
units beside many untreated ones, set `design.one_date_test: permutation`: the
treated units are read against every untreated one, which Welch cannot do with
two or three of them. It needs enough units for a relabelling to reach alpha (3
vs 3 cannot), takes seconds, and is refused with several switch dates (always
permutation), a switchback, or a synthetic control.

With two or more waves, `report.waves` lists each wave's own effect and
interval. They are not adjusted for the number of waves: quote the pooled
headline as the answer, and a clear single wave as a lead, never a finding.

Both take the experiment's only run, both are irreversible, and both refuse
without `--yes` when stdin is not a terminal (exit 8). Confirm with the person
before passing `--yes` — say which design (`design <name>` prints its hash).

## Watching, aborting, reporting

- `status` shows flip counts (`sent` overlaps `confirmed`/`unconfirmed`/`failed`),
  failed flips by unit, guardrail standing, compliance (not measured until a
  treated flip is decided) and the estimate job. `NOT ADVANCING` means the
  driver has not moved the run: that is a platform problem to report, not
  something to fix from the CLI.
- `abort <name> --reason "<what the owner is told>"` with `--levers off`
  (default, the UI's *Turn levers off*: switch treated units back off) or
  `--levers keep` (*Keep levers as they are*). Ask the person which; it is
  their call. It records a request; the next tick actuates it. An aborted run
  cannot resume, its report is labelled ABORTED, and at hour grain only whole
  days before the abort are measured.
- `result` before `reported` prints **sealed** and exits 0. There is
  deliberately no final effect number on a live run — do not try to compute
  one from `status` or from the warehouse. A finished run whose report is not
  written yet prints *being prepared*.
- `interim <name>` is the registered analysis over the periods finished so
  far, on a `live` or `closing` run only (a retrospective run has none). It is
  read once a day while live; `--refresh` asks for a fresh one (members who can
  price; one at a time per run) and `--wait` polls until it lands. It shows
  *read through* the last period it read, *last attempt* when the newest try
  failed (the previous reading stays), and nothing until a whole period of the
  first after window has finished. A run not in progress prints its state and
  exits 0.
- **An interim is a hint, not a result.** Its interval and p-value are the
  registered fixed-horizon ones, correct only when read once at the end; read
  daily, noise crosses the bar far more often. Say *so far* and *not final*
  whenever you quote one, never call it significant, and never advise stopping
  a run or rolling a change out because it looks good. Use it to catch
  something plainly broken (an effect far outside plausible, in the wrong
  direction) and to see data is flowing. It has no total, projection or
  write-back, and it never moves a lever or changes the run.
- A report: state its `verdict` (not a bare "significant"), and repeat every
  line under `assumptions` — a switchback's carryover and pull-forward
  assumptions are part of the result.
- `write-back <name>` opens a branch writing the measured coefficient into the
  World Model file. It needs the deployment that holds the workspace's working
  copy (503 elsewhere); review the branch like any other change.

## Branch on exit codes

`--json` on every verb returns the server's body. Exit codes are the `oxyc`
contract: 2 usage, 4 auth (or no assume session), 5 not found, 6 refused
request (409 stale hash or wrong state, 422 the design's own reason), 7
server/network, 8 an irreversible step not confirmed. The message is the
server's own sentence — quote it to the person rather than paraphrasing.
