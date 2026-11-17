# Product

<!-- impeccable:product-schema 1 -->

## Platform

web

## Users

This record currently covers the **staff admin console** (`/admin`), the surface
the observability redesign targets. Its users are Oxy staff (founders, FDEs,
operators) signed in as Global Admin. They keep many rows on screen at once and
come to answer operational questions about the customer fleet, not to explore.

The customer-facing product (Oxygen: agents, data apps, custom apps, Airhouse,
Airway) has its own users; their record is not captured here yet.

## Product Purpose

The admin console lets a small team run a fleet of customer organizations: who
exists, what each one costs to serve, whether their custom apps work, and what
needs a person. Success is an operator getting the answer from one screen and a
pasteable link, without opening a database console.

## Operating Context

- Fleet size to design for: up to ~100 customer orgs, with a long tail of small
  or idle ones (confirmed 2026-11-17).
- Observability's first question is **who costs most** — rank customers by cost
  to serve (LLM spend plus metered resources) for margin and pricing review.
  Change over time and app health are supporting questions.
- Views are shared as links in threads, so state (window, view, sort) lives in
  the URL.

## Capabilities and Constraints

- Cost to serve: LLM spend is priced per model (direct dollars). App storage and
  egress are direct (public list rates). Function compute is apportioned only
  when `OXY_FLEET_COMPUTE_USD_MONTH` is set; otherwise it is a GB·s quantity.
  OLTP and Airhouse bytes/query time are unmeasured and must be named, never
  drawn as zero. There is deliberately no per-org split of the shared
  infrastructure bill.
- "Not measured" is a third outcome, distinct from zero, throughout.
- Business economics (contract value, FDE hours, margin) live in Accounts
  Insight, not here.
- The redesign may change both the web-app and the `api-observability` crate.

## Brand Commitments

The admin console's conventions in `web-app/src/pages/admin/CLAUDE.md` are
binding: the admin kit (`AdminPage`, `AdminAsync`, `ADMIN_TONE`, `adminNav`),
the hard type scale (`text-xs` default), and **colour on this surface means
something is wrong** — magnitude and composition ride length, position and
lightness, not hue.

## Evidence on Hand

Real fleet data lives on the dev and prod deployments. The PR #4142 screenshots
(`pr-assets/feat/admin-observability-per-org`) use seeded orgs (Poke House, BMG
Industries, July Medical, Gia Foods); treat those as synthetic.

## Product Principles

- Answer "what is out there?" before "which one thing?" — a list before a console.
- Every number traceable to the rows it came from; no invented precision.
- Dollars only where a rate is real; quantities everywhere else.
- Dense and scannable over decorative.
