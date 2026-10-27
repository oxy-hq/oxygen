"""Loads the example workspace's simulated panels."""
import csv
from collections import defaultdict
from pathlib import Path

import os
DB = Path(os.environ.get("EXAMPLES_DB", "../hai-test-examples/.db"))


def shoppers():
    """{shopper: {days_ago: (order_value, full_price_value, orders, redemptions)}}"""
    out = defaultdict(dict)
    with (DB / "experiment_shopper_days.csv").open() as f:
        for r in csv.DictReader(f):
            out[r["shopper_id"]][int(r["days_ago"])] = (
                float(r["order_value"]), float(r["full_price_value"]), int(r["orders"]), int(r["promo_redemptions"]))
    return out


def markets():
    """({market: {days_ago: {sku: revenue}}}, {market: {days_ago: (spend, attributed)}}, {market: size})"""
    sku = defaultdict(lambda: defaultdict(dict))
    with (DB / "ads_market_sku_days.csv").open() as f:
        for r in csv.DictReader(f):
            sku[r["market_id"]][int(r["days_ago"])][r["sku"]] = float(r["revenue"])
    ads = defaultdict(dict)
    with (DB / "ads_market_days.csv").open() as f:
        for r in csv.DictReader(f):
            ads[r["market_id"]][int(r["days_ago"])] = (float(r["ad_spend"]), float(r["ad_attributed_sales"]))
    size = {}
    with (DB / "ads_markets.csv").open() as f:
        for r in csv.DictReader(f):
            size[r["market_id"]] = float(r["market_size"])
    return sku, ads, size


def u(a, b):
    return [f"c{i:02d}" for i in range(a, b + 1)]


def mean(xs):
    xs = list(xs)
    return sum(xs) / len(xs)


def var(xs):
    xs = list(xs)
    m = mean(xs)
    return sum((x - m) ** 2 for x in xs) / (len(xs) - 1)


def window(switch, pre=28, washout=3, post=28, anticipation=0):
    """days_ago ranges (inclusive lists) of the before and after windows."""
    before = list(range(switch + anticipation + pre, switch + anticipation, -1))
    after = list(range(switch - washout, switch - washout - post, -1))
    return before, after


def welch(a, b):
    import math
    ma, mb = mean(a), mean(b)
    va, vb = var(a) / len(a), var(b) / len(b)
    se = math.sqrt(va + vb)
    df = (va + vb) ** 2 / (va ** 2 / (len(a) - 1) + vb ** 2 / (len(b) - 1))
    return ma - mb, se, df
