"""Draws the grain and interim charts: the staggered promo read week by week, its interim
reading day by day as the run goes on, and how often checking every day raises a false alarm."""
import json
import math
import random
import sys
from pathlib import Path

sys.path.insert(0, str(Path(__file__).parent))
from data import mean, shoppers, u, welch  # noqa: E402
from svg import BLUE, Chart, ORANGE, SLATE, TEAL, TEXT  # noqa: E402

OUT = Path(__file__).resolve().parents[2] / "docs/images/experiments"
S = shoppers()
W1, W2, NEVER = u(1, 8), u(9, 16), u(17, 40)


def save(name, chart):
    (OUT / name).write_text(chart.svg())
    print("wrote", name, file=sys.stderr)


# 1. One shopper group by day and by week ----------------------------------
def by_week():
    """Wave 1 and the never-switched shoppers, 28 days either side of the switch (no washout),
    read per day and per 7-day period. The effect is the same money per day either way."""
    sw = 130
    days = list(range(sw + 28, sw - 28, -1))
    diff = {d: mean(S[x][d][0] for x in W1) - mean(S[x][d][0] for x in NEVER) for d in days}
    weeks = [days[i:i + 7] for i in range(0, len(days), 7)]
    wdiff = [(w, sum(diff[d] for d in w)) for w in weeks]
    c = Chart(760, 320, -158, -102, -60, 160, title="Wave 1 minus never-switched: read per day, and per week")
    c.span(-130, -102, TEAL, 0.08, "after the switch", label_y=c.t + 14)
    c.grid([-50, 0, 50, 100, 150], fmt=lambda v: f"${v:g}",
           xticks=[-158, -151, -144, -137, -130, -123, -116, -109, -102], xfmt=lambda v: f"{-v:g}",
           xlabel="days ago")
    c.hline(0, TEXT, dash="2 3", width=1)
    c.line([(-d, diff[d]) for d in days], SLATE, 1.4, opacity=0.8)
    for d in days:
        c.dot(-d, diff[d], SLATE, r=2.2, opacity=0.8)
    for w, v in wdiff:
        c.line([(-w[0] - 0.5, v / 7), (-w[-1] + 0.5, v / 7)], ORANGE, 3)
    c.vline(-130, TEAL)
    c.legend([("One day: difference per shopper-day", SLATE, "line"),
              ("One week: its total ÷ 7, to share the scale", ORANGE, "line")], y=18)
    save("grain-day-vs-week.svg", c)

    def effect(cells):
        before = [x for x in cells if x[0] > sw]
        after = [x for x in cells if x[0] <= sw]
        return mean(v for _, v in after) - mean(v for _, v in before)

    day_effect = effect([(d, diff[d]) for d in days])
    week_effect = effect([(w[0], v) for w, v in wdiff])
    sd_day = math.sqrt(sum((diff[d] - mean(diff.values())) ** 2 for d in days) / (len(days) - 1))
    return {"day_effect": day_effect, "week_effect": week_effect, "week_over_day": week_effect / day_effect,
            "sd_day": sd_day, "zero_share": mean(1.0 if S[x][d][0] == 0 else 0.0 for x in u(1, 40) for d in days)}


# 2. The interim reading, day by day ---------------------------------------
def att(cells, w1, w2, nev):
    out = []
    for wave, d in ((w1, cells[0]), (w2, cells[1])):
        if d is not None:
            out.append(mean(d[x] for x in wave) - mean(d[x] for x in nev))
    return mean(out)


def interim_series():
    """The staggered promo (waves on 130 and 116 days ago, 28-day windows, washout 3) read as the
    interim reads it: the waves with a whole post day before the cut, every post window shortened
    to the shortest read so far, compared by the same 2,000-free reshuffle test (500 here)."""
    switches, pre, washout, post = (130, 116), 28, 3, 28
    U = u(1, 40)
    rng = random.Random(17)
    rows = []
    for cut in range(126, 84, -1):  # days ago of "today": days older than `cut` are read
        read = {sw: (sw - washout) - cut for sw in switches}
        kept = [sw for sw in switches if read[sw] >= 1]
        n = min(post, min(read[sw] for sw in kept))
        cells = []
        for sw in switches:
            if sw not in kept:
                cells.append(None)
                continue
            before = range(sw + pre, sw, -1)
            after = range(sw - washout, sw - washout - n, -1)
            cells.append({x: mean(S[x][d][0] for d in after) - mean(S[x][d][0] for d in before) for x in U})
        obs = att(cells, W1, W2, NEVER)
        nulls = []
        for _ in range(500):
            s = U[:]
            rng.shuffle(s)
            nulls.append(att(cells, s[:8], s[8:16], s[16:]))
        p = (1 + sum(abs(v) >= abs(obs) for v in nulls)) / 501
        spread = math.sqrt(sum(v * v for v in nulls) / len(nulls))
        rows.append({"cut": cut, "days_read": 127 - cut, "post_read": n, "waves": len(kept), "effect": obs,
                     "low": obs - 1.96 * spread, "high": obs + 1.96 * spread, "p": p})
    final = rows[-1]["effect"]
    c = Chart(760, 330, 0, 42, -40, 60, title="The staggered promo's interim reading, one refresh a day")
    c.span(0, 14, SLATE, 0.06, "wave 1 only", label_y=c.h - c.b - 8)
    c.span(14, 42, TEAL, 0.06, "both waves", label_y=c.h - c.b - 8)
    c.grid([-40, -20, 0, 20, 40, 60], fmt=lambda v: f"${v:+g}" if v else "$0",
           xticks=[1, 7, 14, 21, 28, 35, 41], xlabel="days the interim has read since wave 1's after window opened",
           ylabel="effect per shopper-day")
    band_hi = [(r["days_read"], r["high"]) for r in rows]
    band_lo = [(r["days_read"], r["low"]) for r in reversed(rows)]
    pts = " ".join(f"{c.X(x):.1f},{c.Y(y):.1f}" for x, y in band_hi + band_lo)
    c.add(f"<polygon points='{pts}' fill='{TEAL}' fill-opacity='0.14'/>")
    c.hline(0, TEXT, dash="2 3", width=1)
    c.hline(22, BLUE, dash="5 4", width=1.4)
    c.text(41.5, 23.5, "planted truth $22", size=11, color=BLUE, anchor="end")
    c.line([(r["days_read"], r["effect"]) for r in rows], TEAL, 2.6)
    for r in rows:
        if r["p"] < 0.05:
            c.dot(r["days_read"], r["effect"], ORANGE, r=3.6)
    c.legend([("Interim effect", TEAL, "line"), ("range the data allows", TEAL, "span"),
              ("a day it reads 'Passes so far' (p < 0.05)", ORANGE, "box")], y=18)
    save("interim-over-time.svg", c)
    return {"final_interim": final, "first": rows[0], "days_passing": sum(r["p"] < 0.05 for r in rows),
            "looks": len(rows), "wave2_joins_at": next(r["days_read"] for r in rows if r["waves"] == 2),
            "at_7": rows[6], "at_14": rows[13]}


# 3. Checking every day inflates false alarms ------------------------------
def peeking():
    """A panel shaped like the shoppers with no effect at all: how many runs would read
    'significant' on at least one day if someone looked every day, against once at the end."""
    rng = random.Random(23)
    units, days, post = 40, 300, 28
    wk = [0.85, 0.9, 0.95, 1.0, 1.1, 1.3, 1.25]
    base = [rng.lognormvariate(math.log(60), 0.45) for _ in range(units)]
    panel = [[0.0 if rng.random() < 0.15 else b * wk[d % 7] * (1 + 0.12 * math.sin(2 * math.pi * d / 365))
              * (1 + (rng.random() < 0.25)) * rng.lognormvariate(0, 0.35) for d in range(days)] for b in base]
    crit, runs = 2.03, 1000
    ever = [0] * post
    for _ in range(runs):
        sw = rng.randint(28, days - post - 4)
        idx = list(range(units))
        rng.shuffle(idx)
        tr, ct = idx[:16], idx[16:]
        before = [mean(panel[i][sw - 28:sw]) for i in range(units)]
        flagged = False
        for k in range(1, post + 1):
            ch = [mean(panel[i][sw + 3:sw + 3 + k]) - before[i] for i in range(units)]
            e, se, _ = welch([ch[i] for i in tr], [ch[i] for i in ct])
            flagged = flagged or abs(e / se) >= crit
            ever[k - 1] += flagged
    share = [v / runs for v in ever]
    end_only = _end_only(panel, runs, crit, post)
    c = Chart(760, 300, 1, 28, 0, 0.3, title="No effect at all: how often a run looks 'significant'")
    c.grid([0, 0.05, 0.1, 0.15, 0.2, 0.25, 0.3], fmt=lambda v: f"{v * 100:.0f}%",
           xticks=[1, 7, 14, 21, 28], xlabel="days into the after window",
           ylabel="share of runs with a false alarm")
    c.line([(k + 1, s) for k, s in enumerate(share)], ORANGE, 2.8)
    c.hline(end_only, TEAL, dash="", width=2.4)
    c.text(27.8, share[-1] + 0.012, f"looked at every day: {share[-1] * 100:.0f}%", size=12, color=ORANGE,
           anchor="end", weight=600)
    c.text(27.8, end_only - 0.02, f"read once, at the planned end: {end_only * 100:.0f}%", size=12,
           color=TEAL, anchor="end", weight=600)
    save("peeking-false-alarms.svg", c)
    return {"ever_by_end": share[-1], "ever_by_7": share[6], "ever_by_14": share[13], "end_only": end_only}


def _end_only(panel, runs, crit, post):
    rng = random.Random(29)
    units, days = len(panel), len(panel[0])
    hits = 0
    for _ in range(runs):
        sw = rng.randint(28, days - post - 4)
        idx = list(range(units))
        rng.shuffle(idx)
        tr, ct = idx[:16], idx[16:]
        ch = [mean(panel[i][sw + 3:sw + 3 + post]) - mean(panel[i][sw - 28:sw]) for i in range(units)]
        e, se, _ = welch([ch[i] for i in tr], [ch[i] for i in ct])
        hits += abs(e / se) >= crit
    return hits / runs


if __name__ == "__main__":
    json.dump({"by_week": by_week(), "interim": interim_series(), "peeking": peeking()}, sys.stdout, indent=1,
              default=str)
