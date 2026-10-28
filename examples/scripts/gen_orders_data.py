#!/usr/bin/env python3
"""Give the orders demo dataset a real, actionable driver of order value.

Why this exists
---------------
Order value used to be drawn independently of everything else in the dataset.
Mean order value by status came out 541.4 / 539.8 / 545.8 / 541.1 — four
statistically identical numbers — and nothing else about an order predicted what
it was worth either. That is a problem for a demo whose whole pitch is "here is
where your upside is", because there was no upside to find: every "lever" the
Opportunities panel could surface was, necessarily, sampling noise. It duly
surfaced `order_status` as a "+42.7k biggest lever" and the apartment line of
the shipping address as the third.

So this script installs one honest, actionable driver — the sales channel — and
leaves everything else alone, deliberately including `status`, which *should*
have no effect on order value and whose continued flatness is how we check the
significance gate is doing its job on real data rather than just in unit tests.

What it preserves
-----------------
Three invariants hold across the current fixture at 100%, and other measures are
quietly built on them, so this script derives rather than overwrites:

- `orders.total_amount == SUM(qty * unit_price * (1 - discount_percent/100))`
  over the order's line items. `orders.total_order_value` is defined as
  `{{order_items.total_revenue}}` — the line-item side — while `gross_revenue`
  is `SUM(total_amount)`. The two agree to the cent only because of this, and
  `net_revenue` straddles both.
- `orders.tax_amount == round(total_amount * 0.08, 2)`.
- `orders.shipping_cost == SUM(order_shipments.shipping_cost)`, owned entirely
  by `order_shipments.csv`. Untouched here — and the 12,301 orders with no
  shipment rows must stay at 0.0.

Row counts, ids, and every foreign key are preserved exactly. Only `unit_price`
moves; `total_amount` and `tax_amount` are recomputed from it.

Usage
-----
    python3 examples/scripts/gen_orders_data.py [--anchor YYYY-MM-DD] [--check]

`--check` verifies the invariants against what is already on disk and writes
nothing. Deterministic: same seed and anchor in, same bytes out.

If you re-run this with a new anchor, note that `.monitor.yml`'s header comment
quotes a reference date and will not follow it.
"""

from __future__ import annotations

import argparse
import csv
import datetime as dt
import random
from collections import defaultdict
from pathlib import Path

DB = Path(__file__).resolve().parent.parent / ".db"
SEED = 20260717

# ── The signal ──────────────────────────────────────────────────────────────
#
# Channel mix and the effect of each channel on order value. `in_store` is the
# reference; the story is that the mobile app's basket skews cheap because it
# has no cross-sell surface, so it under-monetises traffic it already has. That
# is a real lever: someone owns the mobile merchandising and can go fix it.
#
# Sizing note — the gap has to be big enough to *prove*, not just to exist. A
# city-scoped 90d slice is ~900 orders over 4 channels, so ~225 per segment; at
# an order-value spread of ~360 that is a standard error of ~24 per segment and
# ~34 on the difference. The significance gate at k=4 asks for ~2.24 SE, i.e.
# ~76. The mobile gap lands near 150 — comfortably provable in a single city,
# without being so cartoonish that the demo looks staged.
CHANNEL_MIX = {"in_store": 0.38, "online": 0.34, "mobile_app": 0.21, "phone": 0.07}
CHANNEL_LIFT = {"in_store": 1.00, "phone": 0.96, "online": 0.92, "mobile_app": 0.72}

# Per-store multiplier spread. Real chains differ store to store, and this is
# what makes `city` / `store_name` a genuine lever when you size across the
# whole population. It cancels out inside a single-store city, which is correct:
# scoped to one store there is nothing left to compare it against.
STORE_SIGMA = 0.06


def _rows(name: str) -> list[dict]:
    with open(DB / name, newline="") as f:
        return list(csv.DictReader(f))


def _write(name: str, rows: list[dict], fields: list[str]) -> None:
    with open(DB / name, "w", newline="") as f:
        w = csv.DictWriter(f, fieldnames=fields)
        w.writeheader()
        w.writerows(rows)


def line_total(li: dict) -> float:
    return (
        int(li["quantity"])
        * float(li["unit_price"])
        * (1 - float(li["discount_percent"]) / 100)
    )


def check(orders: list[dict], items_by_order: dict[str, list[dict]]) -> list[str]:
    """Return a list of invariant violations, empty when the fixture is sound."""
    bad = []
    n_li = sum(
        1
        for o in orders
        if abs(
            float(o["total_amount"])
            - sum(line_total(li) for li in items_by_order.get(o["id"], []))
        )
        >= 0.01
    )
    if n_li:
        bad.append(f"{n_li} orders where total_amount != sum(line items)")
    n_tax = sum(
        1
        for o in orders
        if abs(float(o["tax_amount"]) - round(float(o["total_amount"]) * 0.08, 2)) >= 0.01
    )
    if n_tax:
        bad.append(f"{n_tax} orders where tax_amount != round(total_amount*0.08, 2)")
    return bad


def main() -> None:
    ap = argparse.ArgumentParser(description=__doc__)
    ap.add_argument(
        "--anchor",
        help="Shift every date so the newest order lands on this day "
        "(default: yesterday, so the default 'Last 90d' window is fully covered "
        "and today's partial day never reads as a slump).",
    )
    ap.add_argument("--check", action="store_true", help="Verify invariants; write nothing.")
    args = ap.parse_args()

    orders = _rows("orders.csv")
    items = _rows("order_items.csv")
    items_by_order: dict[str, list[dict]] = defaultdict(list)
    for li in items:
        items_by_order[li["order_id"]].append(li)

    if args.check:
        bad = check(orders, items_by_order)
        print("\n".join(bad) if bad else "invariants hold")
        raise SystemExit(1 if bad else 0)

    rng = random.Random(SEED)

    # Per-store multiplier, drawn once and keyed by store so it is stable.
    store_factor = {
        s: rng.gauss(1.0, STORE_SIGMA) for s in sorted({o["store_id"] for o in orders})
    }

    channels = list(CHANNEL_MIX)
    weights = [CHANNEL_MIX[c] for c in channels]

    for o in orders:
        channel = rng.choices(channels, weights=weights, k=1)[0]
        o["channel"] = channel
        factor = CHANNEL_LIFT[channel] * store_factor[o["store_id"]]
        # Scale the order's line items, then derive the order's totals back from
        # them — never the other way round, or total_order_value (line-item
        # side) and gross_revenue (total_amount side) silently diverge.
        for li in items_by_order[o["id"]]:
            li["unit_price"] = f"{float(li['unit_price']) * factor:.2f}"
        total = round(sum(line_total(li) for li in items_by_order[o["id"]]), 2)
        o["total_amount"] = f"{total:.2f}"
        o["tax_amount"] = f"{round(total * 0.08, 2):.2f}"

    date_files = shift_dates(orders, args.anchor)

    _write("orders.csv", orders, list(orders[0].keys()))
    _write("order_items.csv", items, list(items[0].keys()))
    for name, rows in date_files.items():
        _write(name, rows, list(rows[0].keys()))

    bad = check(orders, items_by_order)
    if bad:
        raise SystemExit("invariants broken by this run:\n" + "\n".join(bad))
    report(orders)


def shift_dates(orders: list[dict], anchor: str | None) -> dict[str, list[dict]]:
    """Translate every date in the dataset by one constant offset.

    A pure translation, so every relative relationship — ordered before shipped
    before delivered, returns trailing their orders — survives untouched. The
    alternative (regenerating dates) would have to reinvent all of those.
    """
    target = (
        dt.date.fromisoformat(anchor)
        if anchor
        else dt.date.today() - dt.timedelta(days=1)
    )
    newest = max(dt.date.fromisoformat(o["order_date"]) for o in orders)
    delta = dt.timedelta(days=(target - newest).days)
    if not delta:
        return {}

    def bump(row: dict, col: str) -> None:
        if row.get(col):
            row[col] = (dt.date.fromisoformat(row[col]) + delta).isoformat()

    for o in orders:
        bump(o, "order_date")

    out: dict[str, list[dict]] = {}
    for name, cols in (
        ("order_shipments.csv", ("shipped_date", "estimated_delivery", "actual_delivery")),
        ("order_returns.csv", ("return_date",)),
    ):
        rows = _rows(name)
        for r in rows:
            for c in cols:
                bump(r, c)
        out[name] = rows
    print(f"shifted every date by {delta.days:+d}d (newest order → {target})")
    return out


def report(orders: list[dict]) -> None:
    """Print what a reader should now be able to find in the data."""
    by: dict[str, list[float]] = defaultdict(list)
    for o in orders:
        by[o["channel"]].append(float(o["total_amount"]))
    print(f"\n{'channel':<12}{'orders':>9}{'avg order value':>18}")
    for c, vals in sorted(by.items(), key=lambda kv: -sum(kv[1]) / len(kv[1])):
        print(f"{c:<12}{len(vals):>9,}{sum(vals) / len(vals):>18,.2f}")

    st: dict[str, list[float]] = defaultdict(list)
    for o in orders:
        st[o["status"]].append(float(o["total_amount"]))
    print(f"\n{'status':<12}{'orders':>9}{'avg order value':>18}   (flat on purpose)")
    for s, vals in sorted(st.items()):
        print(f"{s:<12}{len(vals):>9,}{sum(vals) / len(vals):>18,.2f}")


if __name__ == "__main__":
    main()
