#!/usr/bin/env python3
"""Seed data for the experiment examples in examples/experiments/.

Why this exists
---------------
An experiment report is only checkable against a known truth, so every past
change these files hold is PLANTED at a stated size, and the examples in
examples/experiments/ read them back. The truth table is printed at the end of
a run and repeated in examples/experiments/README.md; keep the two in sync.

Two panels, one per kind of unit an experiment randomises over:

  exp_stores / exp_store_days / exp_store_hours
      48 stores in 4 regions, daily from 2025-11-03 and hourly from
      2026-09-01, both through 2026-11-01. Sizes are lognormal, days share a
      regional factor, so the arms of a draw differ the way real stores do.

  exp_skus / exp_sku_days
      90 SKUs in 6 categories of 15, 18 brands of 5, daily from 2026-04-06
      through 2026-11-01. Traffic is power-law across SKUs, every SKU shares
      the week's shape, and every SKU in a category moves with that
      category's own AR(1) factor: the shared movement a synthetic control
      builds its donors from.

What is planted
---------------
  Spring sale, one date with an end (stores, observed)
      S02 S04 S09 S10 S17 S18 S20 S35 S39 S40 S43 S45, three per region:
      +10% in-store revenue from 2026-04-06 until it ended on 2026-04-20,
      then -6% for 14 days (pull-forward: shoppers who bought early).
  Self-checkout, staggered (stores, observed)
      +6% in-store revenue and transactions from each wave's date on:
      2026-07-06 S03 S11 S19 S27 | 2026-08-03 S05 S13 S21 S29 |
      2026-08-31 S07 S15 S23 S31.
  Double-points weeks, switchback (stores, every store)
      +6% revenue in the weeks starting 2026-09-07, 09-28, 10-05, 10-26
      (the 'on' weeks of four on/off pairs from 2026-09-07).
  Labor Day kit deal, one date (SKUs, observed)
      KIT-02 KIT-07 KIT-11: conversion x1.40 from 2026-09-07 until it ended on
      2026-09-21. Siblings KIT-03 and KIT-08 lose 10% conversion while it runs
      (cannibalised), so they must be kept out of a synthetic control's donors.
  Search badge, one date (SKUs, observed)
      TOL-01 TOL-04 TOL-06 TOL-09 TOL-12 PWR-02 PWR-05 PWR-08 PWR-11 PWR-14:
      conversion (units / page views) x1.15 from 2026-06-15 on; traffic is
      untouched.

Nothing is planted after 2026-11-01, so the pre-registered drafts price
against history that holds only the changes above.

Deterministic: the same seed writes the same bytes.

    python3 examples/scripts/gen_experiments_data.py
"""

from __future__ import annotations

import argparse
import csv
import datetime as dt
import math
import random
from pathlib import Path

OUT = Path(__file__).resolve().parent.parent / ".db"

STORE_START = dt.date(2025, 11, 3)
HOUR_START = dt.date(2026, 9, 1)
SKU_START = dt.date(2026, 4, 6)
END = dt.date(2026, 11, 1)

REGIONS = ["north", "south", "east", "west"]
WEEKDAY = [0.92, 0.88, 0.9, 0.97, 1.12, 1.25, 1.06]  # Monday first
# Online SKU traffic swings harder over the week. Every SKU shares it, which
# is most of what a synthetic control's donors track a treated SKU by.
SKU_WEEKDAY = [0.75, 0.8, 0.85, 0.95, 1.15, 1.45, 1.05]

SELF_CHECKOUT = {
    dt.date(2026, 7, 6): ["S03", "S11", "S19", "S27"],
    dt.date(2026, 8, 3): ["S05", "S13", "S21", "S29"],
    dt.date(2026, 8, 31): ["S07", "S15", "S23", "S31"],
}
SELF_CHECKOUT_LIFT = 1.06
DOUBLE_POINTS_WEEKS = [
    dt.date(2026, 9, 7),
    dt.date(2026, 9, 28),
    dt.date(2026, 10, 5),
    dt.date(2026, 10, 26),
]
DOUBLE_POINTS_LIFT = 1.06
SPRING_SALE = [
    "S02", "S04", "S09", "S10", "S17", "S18",
    "S20", "S35", "S39", "S40", "S43", "S45",
]
SPRING_SALE_ON = dt.date(2026, 4, 6)
SPRING_SALE_OFF = dt.date(2026, 4, 20)
SPRING_SALE_LIFT = 1.10
SPRING_SALE_AFTER = 0.94
SPRING_SALE_AFTER_DAYS = 14

CATEGORIES = [
    ("kits", "KIT"),
    ("tools", "TOL"),
    ("safety", "SAF"),
    ("power", "PWR"),
    ("storage", "STO"),
    ("lighting", "LIT"),
]
DEAL_SKUS = ["KIT-02", "KIT-07", "KIT-11"]
DEAL_SIBLINGS = ["KIT-03", "KIT-08"]
DEAL_ON = dt.date(2026, 9, 7)
DEAL_OFF = dt.date(2026, 9, 21)
DEAL_LIFT = 1.40
CANNIBALISED = 0.90
BADGE_SKUS = [
    "TOL-01", "TOL-04", "TOL-06", "TOL-09", "TOL-12",
    "PWR-02", "PWR-05", "PWR-08", "PWR-11", "PWR-14",
]
BADGE_ON = dt.date(2026, 6, 15)
BADGE_LIFT = 1.15


def days(start: dt.date, end: dt.date):
    d = start
    while d <= end:
        yield d
        d += dt.timedelta(days=1)


def ar1(rng: random.Random, n: int, phi: float, sd: float) -> list[float]:
    """A multiplicative AR(1) factor around 1."""
    x, out = 0.0, []
    for _ in range(n):
        x = phi * x + rng.gauss(0, sd)
        out.append(math.exp(x))
    return out


def poisson(rng: random.Random, lam: float) -> int:
    if lam > 60:
        return max(0, round(rng.gauss(lam, math.sqrt(lam))))
    l, k, p = math.exp(-lam), 0, 1.0
    while True:
        p *= rng.random()
        if p <= l:
            return k
        k += 1


def write(name: str, header: list[str], rows: list[list]) -> None:
    with open(OUT / name, "w", newline="") as f:
        w = csv.writer(f)
        w.writerow(header)
        w.writerows(rows)
    print(f"wrote .db/{name}: {len(rows)} rows")


# --- stores -----------------------------------------------------------------


def store_list(rng: random.Random) -> list[dict]:
    stores = []
    for i in range(1, 49):
        size = math.exp(rng.gauss(math.log(3200), 0.45))
        stores.append(
            {
                "code": f"S{i:02d}",
                "name": f"Store {i:02d}",
                "region": REGIONS[(i - 1) % 4],
                "size": size,
                "online_share": rng.uniform(0.12, 0.3),
                "basket": rng.uniform(28, 42),
            }
        )
    return stores


def self_checkout_since(code: str) -> dt.date | None:
    for when, units in SELF_CHECKOUT.items():
        if code in units:
            return when
    return None


def spring_sale(code: str, day: dt.date) -> float:
    if code not in SPRING_SALE:
        return 1.0
    if SPRING_SALE_ON <= day < SPRING_SALE_OFF:
        return SPRING_SALE_LIFT
    if SPRING_SALE_OFF <= day < SPRING_SALE_OFF + dt.timedelta(days=SPRING_SALE_AFTER_DAYS):
        return SPRING_SALE_AFTER
    return 1.0


def double_points(day: dt.date) -> bool:
    return any(w <= day < w + dt.timedelta(days=7) for w in DOUBLE_POINTS_WEEKS)


def store_days(rng: random.Random, stores: list[dict]) -> list[list]:
    calendar = list(days(STORE_START, END))
    regional = {r: ar1(rng, len(calendar), 0.8, 0.012) for r in REGIONS}
    rows = []
    for s in stores:
        since = self_checkout_since(s["code"])
        for t, day in enumerate(calendar):
            base = s["size"] * WEEKDAY[day.weekday()] * regional[s["region"]][t]
            base *= math.exp(rng.gauss(0, 0.07))
            sc = since is not None and day >= since
            dp = double_points(day)
            lift = DOUBLE_POINTS_LIFT if dp else 1.0
            instore = base * (1 - s["online_share"]) * lift
            online = base * s["online_share"] * lift * math.exp(rng.gauss(0, 0.1))
            if sc:
                instore *= SELF_CHECKOUT_LIFT
            instore *= spring_sale(s["code"], day)
            transactions = poisson(rng, instore / s["basket"])
            refunds = (instore + online) * rng.uniform(0.01, 0.03)
            rows.append(
                [
                    s["code"],
                    day.isoformat(),
                    round(instore, 2),
                    round(online, 2),
                    transactions,
                    round(refunds, 2),
                    int(sc),
                    int(dp),
                ]
            )
    return rows


HOUR_SHAPE = [
    0.15, 0.1, 0.08, 0.08, 0.1, 0.2, 0.45, 0.8, 1.0, 1.05, 1.15, 1.35,
    1.5, 1.4, 1.2, 1.15, 1.3, 1.55, 1.6, 1.4, 1.1, 0.8, 0.5, 0.3,
]


def store_hours(rng: random.Random, stores: list[dict]) -> list[list]:
    rows = []
    total = sum(HOUR_SHAPE)
    for s in stores:
        for day in days(HOUR_START, END):
            day_rev = s["size"] * WEEKDAY[day.weekday()]
            for h, share in enumerate(HOUR_SHAPE):
                rev = day_rev * share / total * math.exp(rng.gauss(0, 0.15))
                at = dt.datetime.combine(day, dt.time(h))
                rows.append(
                    [
                        s["code"],
                        at.isoformat(sep=" "),
                        round(rev, 2),
                        poisson(rng, rev / s["basket"]),
                        0,
                    ]
                )
    return rows


# --- SKUs -------------------------------------------------------------------


def sku_list(rng: random.Random) -> list[dict]:
    skus = []
    for c, (category, prefix) in enumerate(CATEGORIES):
        # Traffic is power-law, but each category's deal or badge SKUs sit inside
        # the range of the others, so a synthetic control can reach them.
        views = sorted(
            (math.exp(rng.gauss(math.log(400), 0.9)) for _ in range(15)),
            reverse=True,
        )
        rng.shuffle(views)
        for i in range(1, 16):
            code = f"{prefix}-{i:02d}"
            v = views[i - 1]
            if code in DEAL_SKUS:
                v = [1400.0, 1100.0, 900.0][DEAL_SKUS.index(code)]
            brand = f"{category}_{'abc'[(i - 1) // 5]}"
            skus.append(
                {
                    "sku": code,
                    "name": f"{category.title()} item {i:02d}",
                    "category": category,
                    "brand": brand,
                    "category_index": c,
                    "views": v,
                    "conversion": rng.uniform(0.04, 0.08),
                    "price": round(rng.uniform(12, 180), 2),
                }
            )
    return skus


def conversion_lift(sku: str, day: dt.date) -> tuple[float, bool]:
    """The planted multiplier on conversion, and whether a promo is live."""
    if sku in DEAL_SKUS:
        if DEAL_ON <= day < DEAL_OFF:
            return DEAL_LIFT, True
    if sku in DEAL_SIBLINGS and DEAL_ON <= day < DEAL_OFF:
        return CANNIBALISED, False
    if sku in BADGE_SKUS and day >= BADGE_ON:
        return BADGE_LIFT, True
    return 1.0, False


def sku_days(rng: random.Random, skus: list[dict]) -> list[list]:
    calendar = list(days(SKU_START, END))
    category = [ar1(rng, len(calendar), 0.9, 0.03) for _ in CATEGORIES]
    trend = [1 + 0.15 * math.sin(2 * math.pi * t / 365) for t in range(len(calendar))]
    rows = []
    for s in skus:
        own = ar1(rng, len(calendar), 0.5, 0.03)
        for t, day in enumerate(calendar):
            traffic = (
                s["views"]
                * SKU_WEEKDAY[day.weekday()]
                * category[s["category_index"]][t]
                * trend[t]
                * own[t]
            )
            page_views = poisson(rng, traffic)
            lift, on_promo = conversion_lift(s["sku"], day)
            units = poisson(rng, page_views * s["conversion"] * lift)
            rows.append(
                [
                    s["sku"],
                    day.isoformat(),
                    page_views,
                    units,
                    round(units * s["price"], 2),
                    int(on_promo),
                ]
            )
    return rows


def main() -> None:
    p = argparse.ArgumentParser(description=__doc__.split("\n\n")[0])
    p.add_argument("--seed", type=int, default=20261102)
    args = p.parse_args()
    rng = random.Random(args.seed)

    stores = store_list(rng)
    write(
        "exp_stores.csv",
        ["store_code", "store_name", "region"],
        [[s["code"], s["name"], s["region"]] for s in stores],
    )
    write(
        "exp_store_days.csv",
        [
            "store_code",
            "business_date",
            "instore_revenue",
            "online_revenue",
            "transactions",
            "refunds",
            "self_checkout_live",
            "double_points_live",
        ],
        store_days(rng, stores),
    )
    write(
        "exp_store_hours.csv",
        ["store_code", "hour_start", "revenue", "transactions", "kiosk_prompt_live"],
        store_hours(rng, stores),
    )

    skus = sku_list(rng)
    write(
        "exp_skus.csv",
        ["sku", "sku_name", "category", "brand", "list_price"],
        [[s["sku"], s["name"], s["category"], s["brand"], s["price"]] for s in skus],
    )
    write(
        "exp_sku_days.csv",
        ["sku", "sale_date", "page_views", "units", "revenue", "on_promo"],
        sku_days(rng, skus),
    )


if __name__ == "__main__":
    main()
