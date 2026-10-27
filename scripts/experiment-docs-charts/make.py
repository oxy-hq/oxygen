"""Draws the experiments docs' charts from the example workspace's data."""
import json
import math
import random
import sys
from pathlib import Path

sys.path.insert(0, str(Path(__file__).parent))
from data import mean, markets, shoppers, u, welch, window  # noqa: E402
from svg import BLUE, Chart, ORANGE, RED, SLATE, TEAL, TEXT  # noqa: E402

OUT = Path(__file__).resolve().parents[2] / "docs/images/experiments"
OUT.mkdir(parents=True, exist_ok=True)
S = shoppers()


def save(name, chart):
    (OUT / name).write_text(chart.svg())
    print("wrote", name)


def rolling(series, k=7):
    out = []
    for i in range(len(series)):
        if i >= k - 1:
            out.append((series[i][0], mean(v for _, v in series[i - k + 1:i + 1])))
    return out


def daily(units, days, col=0):
    # x axis = days ago, drawn left (older) to right (newer) as negative days
    return [(-d, mean(S[x][d][col] for x in units)) for d in days]


# 1. Staggered rollout, day by day ---------------------------------------
def rollout_series():
    days = list(range(175, 59, -1))
    w1, w2, ctl = rolling(daily(u(1, 8), days)), rolling(daily(u(9, 16), days)), rolling(daily(u(17, 40), days))
    c = Chart(760, 340, -170, -60, 40, 170, title="Staggered rollout: 7-day average order value per shopper")
    c.span(-127, -100, TEAL, 0.10, "wave 1 after window", label_y=c.h - c.b - 8)
    c.span(-113, -86, BLUE, 0.08, "wave 2 after window", label_y=c.h - c.b - 22)
    c.grid([40, 70, 100, 130, 160], fmt=lambda v: f"${v:g}", xticks=[-170, -150, -130, -116, -100, -80, -60],
           xfmt=lambda v: f"{-v:g}", xlabel="days ago")
    c.vline(-130, TEAL, label=None)
    c.vline(-116, BLUE)
    c.vline(-80, TEXT)
    c.text(-80 + 1.5, 160, "promo off", size=11)
    c.line(ctl, SLATE, 2.2)
    c.line(w1, TEAL, 2.6)
    c.line(w2, BLUE, 2.6)
    c.legend([("Wave 1 (8 shoppers, on 130 days ago)", TEAL, "line"), ("Wave 2 (8, on 116 days ago)", BLUE, "line"),
              ("Never switched (24)", SLATE, "line")], y=18)
    save("rollout-series.svg", c)


# 2. Each unit's change, wave 1 ------------------------------------------
def unit_changes():
    b, a = window(130)
    ch = {x: mean(S[x][d][0] for d in a) - mean(S[x][d][0] for d in b) for x in u(1, 40)}
    tr, ct = [ch[x] for x in u(1, 8)], [ch[x] for x in u(17, 40)]
    c = Chart(760, 300, 0, 10, -70, 60, left=64, bottom=30, title="Each shopper's change, after minus before")
    c.grid([-60, -30, 0, 30, 60], fmt=lambda v: f"{v:+g}" if v else "0")
    c.hline(0, TEXT, dash="2 3", width=1)
    rng = random.Random(3)
    for v in ct:
        c.dot(2.5 + rng.uniform(-0.9, 0.9), v, SLATE, r=4, opacity=0.8)
    for v in tr:
        c.dot(7.5 + rng.uniform(-0.9, 0.9), v, TEAL, r=4.5)
    mt, mc = mean(tr), mean(ct)
    c.line([(1.2, mc), (3.8, mc)], SLATE, 3)
    c.line([(6.2, mt), (8.8, mt)], TEAL, 3)
    c.text(3.9, mc - 1, f"average {mc:+.1f}", size=12, color=TEXT)
    c.text(8.9, mt - 1, f"average {mt:+.1f}", size=12, color=TEAL, weight=600)
    c.line([(5, mc), (5, mt)], ORANGE, 2.5)
    c.text(5.15, (mc + mt) / 2 - 1, f"effect ≈ {mt - mc:+.1f}", size=13, color=ORANGE, weight=600)
    c.text(2.5, -66, "24 shoppers who never got the promo", size=12, anchor="middle")
    c.text(7.5, -66, "8 shoppers in wave 1", size=12, anchor="middle")
    save("unit-changes.svg", c)
    return mt - mc


# 3. Shuffle (permutation) distribution ---------------------------------
def permutation():
    U = u(1, 40)

    def dd(x, sw):
        b, a = window(sw)
        return mean(S[x][d][0] for d in a) - mean(S[x][d][0] for d in b)

    D1, D2 = {x: dd(x, 130) for x in U}, {x: dd(x, 116) for x in U}

    def att(w1, w2, nev):
        a1 = mean(D1[x] for x in w1) - mean(D1[x] for x in nev)
        a2 = mean(D2[x] for x in w2) - mean(D2[x] for x in nev)
        return (a1 + a2) / 2

    obs = att(u(1, 8), u(9, 16), u(17, 40))
    r = random.Random(11)
    nulls = []
    for _ in range(2000):
        s = U[:]
        r.shuffle(s)
        nulls.append(att(s[:8], s[8:16], s[16:]))
    k = sum(abs(v) >= abs(obs) for v in nulls)
    p = (1 + k) / 2001
    lo, hi, w = -24, 24, 2
    bins = [0] * int((hi - lo) / w)
    for v in nulls:
        i = int((v - lo) // w)
        if 0 <= i < len(bins):
            bins[i] += 1
    c = Chart(760, 300, lo, hi, 0, max(bins) * 1.15, bottom=44,
              title="2,000 reshuffles of who got the promo, and the real result")
    c.grid([], xticks=list(range(-24, 25, 6)), xfmt=lambda v: f"{v:+g}" if v else "0",
           xlabel="effect the analysis would report if the promo did nothing ($ per shopper-day)")
    for i, n in enumerate(bins):
        x0 = lo + i * w
        extreme = abs(x0 + w / 2) >= abs(obs)
        c.rect(x0 + 0.1, x0 + w - 0.1, 0, n, ORANGE if extreme else SLATE, 0.85 if extreme else 0.55)
    c.vline(obs, TEAL, dash="", width=2.5)
    c.text(obs + 0.6, max(bins) * 1.0, f"real result {obs:+.1f}", size=12, color=TEAL, weight=600)
    c.text(lo + 0.5, max(bins) * 1.05, f"only {k} of 2,000 shuffles are as extreme (orange): p ≈ {p:.3f}",
           size=12, color=TEXT)
    save("permutation-null.svg", c)
    return obs, p


# 4. Local shock and the pre-trend check ---------------------------------
def local_shock():
    days = list(range(365, 279, -1))
    tr, ct = rolling(daily(u(1, 10), days)), rolling(daily(u(11, 20), days))
    c = Chart(760, 330, -360, -280, 30, 200, title="A shock in the before window, caught by the pre-trend check")
    c.span(-346, -333, SLATE, 0.10, "before, 1st half", label_y=c.t + 14)
    c.span(-332, -319, ORANGE, 0.12, "before, 2nd half", label_y=c.t + 14)
    c.span(-315, -288, TEAL, 0.10, "after window", label_y=c.t + 14)
    c.grid([40, 80, 120, 160, 200], fmt=lambda v: f"${v:g}", xticks=[-360, -346, -332, -318, -300, -280],
           xfmt=lambda v: f"{-v:g}", xlabel="days ago")
    c.vline(-332, ORANGE)
    c.text(-331.4, 48, "unrelated +$70 shock starts", size=11, color=ORANGE)
    c.vline(-318, TEAL)
    c.text(-317.4, 35, "promo switch (does nothing)", size=11, color=TEAL)
    c.line(ct, SLATE, 2.2)
    c.line(tr, TEAL, 2.6)
    c.legend([("Treated (10 shoppers)", TEAL, "line"), ("Control (10 shoppers)", SLATE, "line")], y=18)
    save("local-shock-pretrend.svg", c)


# 5. Switchback periods ---------------------------------------------------
def switchback():
    F = u(1, 40)
    pat = ["on", "off", "off", "on", "on", "off", "off", "on"]
    vals = []
    for i in range(8):
        start = 250 - 7 * i
        vals.append(mean(S[x][d][0] for x in F for d in [start - k for k in range(2, 7)]))
    c = Chart(760, 320, 0, 8, 60, 112, bottom=56, title="Switchback: fleet average per period, in coin-flipped pairs")
    c.grid([60, 70, 80, 90, 100, 110], fmt=lambda v: f"${v:g}")
    diffs = []
    for i, (st, v) in enumerate(zip(pat, vals)):
        c.rect(i + 0.12, i + 0.88, 60, v, TEAL if st == "on" else SLATE, 0.85 if st == "on" else 0.5)
        c.text(i + 0.5, v + 1.2, f"${v:.0f}", size=11, anchor="middle")
        c.text(i + 0.5, 57.5, f"{st}", size=11, anchor="middle", color=TEAL if st == "on" else TEXT)
    for p in range(4):
        a, b = 2 * p, 2 * p + 1
        on, off = (vals[a], vals[b]) if pat[a] == "on" else (vals[b], vals[a])
        diffs.append(on - off)
        y = c.Y(60) + 34
        c.add(f"<path d='M{c.X(a + 0.1):.1f},{y - 8:.1f} V{y:.1f} H{c.X(b + 0.9):.1f} V{y - 8:.1f}' fill='none' stroke='{TEXT}' stroke-width='1'/>")
        c.text((c.X(a + 0.1) + c.X(b + 0.9)) / 2, y + 14, f"pair {p + 1}: on − off = {on - off:+.1f}", size=11,
               anchor="middle", px=True, color=ORANGE, weight=600)
    c.legend([("On period", TEAL, "box"), ("Off period", SLATE, "box")], y=18)
    save("switchback-periods.svg", c)
    return diffs


# 6. Geo lift: what the ads did ------------------------------------------
def geo_lift():
    sku, ads, _ = markets()
    M = sorted(ads)
    T = ["atlanta", "charlotte", "denver", "kansas_city", "miami", "new_york", "phoenix", "san_diego"]
    C = [m for m in M if m not in T]
    b, a = window(100)

    def did(f):
        def dm(m):
            return mean(f(m, d) for d in a) - mean(f(m, d) for d in b)
        return welch([dm(m) for m in T], [dm(m) for m in C])

    hero = did(lambda m, d: sku[m][d]["TB-40"])
    sib = did(lambda m, d: sum(sku[m][d][s] for s in ("TB-30", "SM-35", "DP-20")))
    spend = did(lambda m, d: ads[m][d][0])
    attr = did(lambda m, d: ads[m][d][1])
    rows = [("Ad spend", spend[0], SLATE), ("Sales the ads platform credits", attr[0], BLUE),
            ("TB-40 sales (measured lift)", hero[0], TEAL), ("Other backpacks", sib[0], RED),
            ("All backpacks together", hero[0] + sib[0], ORANGE)]
    c = Chart(760, 300, -100, 170, 0, len(rows), left=230, right=40, top=20, bottom=40,
              title="Ads on TB-40: change per market-day in the 8 ad markets, against the 16 others")
    c.grid([], xticks=[-100, -50, 0, 50, 100, 150], xfmt=lambda v: ("+$" if v > 0 else "−$") + f"{abs(v):g}" if v else "$0",
           xlabel="change per market-day")
    for i, (label, v, col) in enumerate(rows):
        y = len(rows) - i - 0.5
        c.rect(0, v, y - 0.32, y + 0.32, col, 0.85)
        c.text(c.l - 10, c.Y(y) + 4, label, size=12, anchor="end", px=True)
        c.text(v + (4 if v >= 0 else -4), y - 0.12, f"{v:+.0f}", size=12, anchor="start" if v >= 0 else "end",
               color=col, weight=600)
    c.vline(0, TEXT, dash="", width=1)
    save("geo-lift-breakdown.svg", c)
    return hero, sib, spend, attr


# 7. Predicted against measured -----------------------------------------
def predicted(hero, spend):
    est, se, df = hero
    half = 2.08 * se
    pred = 3 * spend[0]
    c = Chart(760, 220, 0, 160, 0, 2, left=230, right=40, top=20, bottom=40,
              title="Predicted against measured, TB-40 ads")
    c.grid([], xticks=[0, 40, 80, 120, 160], xfmt=lambda v: f"${v:g}", xlabel="TB-40 sales per market-day")
    c.rect(est - half, est + half, 0.75, 1.25, TEAL, 0.18)
    c.dot(est, 1.0, TEAL, r=6)
    c.text(est, 1.38, f"measured {est:+.0f} (range {est - half:.0f} to {est + half:.0f})", size=12, anchor="middle",
           color=TEAL, weight=600)
    c.dot(pred, 1.0, RED, r=6)
    c.text(pred, 0.48, f"predicted {pred:.0f} = $3 × {spend[0]:.1f} spend", size=12, anchor="middle", color=RED,
           weight=600)
    c.text(c.l - 10, c.Y(1.0) + 4, "TB-40 lift per market-day", size=12, anchor="end", px=True)
    save("predicted-vs-measured.svg", c)


# 8. Matched pairs over 24 markets ---------------------------------------
def matched_pairs():
    _, _, size = markets()
    ranked = sorted(size, key=size.get, reverse=True)
    r = random.Random(42)
    treated = set()
    for i in range(0, 24, 2):
        treated.add(ranked[i + r.randint(0, 1)])
    c = Chart(760, 330, 0, 24, 0, 1.9, left=40, bottom=96, top=40,
              title="Matched pairs: 24 markets ranked by size, one of each pair gets the change")
    c.grid([0.5, 1.0, 1.5], fmt=lambda v: f"{v:g}×", ylabel="market size")
    for i, m in enumerate(ranked):
        col = TEAL if m in treated else SLATE
        c.rect(i + 0.12, i + 0.88, 0, size[m], col, 0.85 if m in treated else 0.5)
        X, Y = c.X(i + 0.5), c.Y(0) + 8
        c.add(f"<text transform='translate({X:.1f},{Y:.1f}) rotate(55)' font-size='10' fill='{TEXT}' "
              f"font-family='Inter, system-ui, sans-serif'>{m.replace('_', ' ')}</text>")
        if i % 2 == 0:
            y = c.Y(max(size[m], size[ranked[i + 1]])) - 8
            c.add(f"<path d='M{c.X(i + 0.2):.1f},{y + 5:.1f} V{y:.1f} H{c.X(i + 1.8):.1f} V{y + 5:.1f}' "
                  f"fill='none' stroke='{TEXT}' stroke-width='1'/>")
    c.legend([("Gets the change", TEAL, "box"), ("Control", SLATE, "box")], y=20)
    save("matched-pairs.svg", c)


# 9. Placebo power on a simulated panel ----------------------------------
def placebo_power():
    rng = random.Random(5)
    units, days = 40, 300
    wk = [0.85, 0.9, 0.95, 1.0, 1.1, 1.3, 1.25]
    base = [rng.lognormvariate(math.log(60), 0.45) for _ in range(units)]
    panel = []
    for b in base:
        row = []
        for d in range(days):
            v = 0.0 if rng.random() < 0.15 else b * wk[d % 7] * (1 + 0.12 * math.sin(2 * math.pi * d / 365)) * \
                (1 + (rng.random() < 0.25)) * rng.lognormvariate(0, 0.35)
            row.append(v)
        panel.append(row)
    draws = []
    for _ in range(600):
        sw = rng.randint(28, days - 31)
        idx = list(range(units))
        rng.shuffle(idx)
        tr, ct = idx[:16], idx[16:]
        ch = [mean(panel[i][sw + 3:sw + 31]) - mean(panel[i][sw - 28:sw]) for i in range(units)]
        draws.append(welch([ch[i] for i in tr], [ch[i] for i in ct]))
    crit = 2.03

    def power(tau):
        return mean(1.0 if abs((e + tau) / se) >= crit else 0.0 for e, se, _ in draws)

    taus = [t / 2 for t in range(0, 61)]
    curve = [(t, power(t)) for t in taus]
    mde = next(t for t, p in curve if p >= 0.8)
    c = Chart(760, 300, 0, 30, 0, 1, title="Placebo power: how often each effect size would be caught")
    c.grid([0, 0.25, 0.5, 0.8, 1.0], fmt=lambda v: f"{v * 100:.0f}%", xticks=[0, 5, 10, 15, 20, 25, 30],
           xfmt=lambda v: f"${v:g}", xlabel="effect added to the fake treated group ($ per shopper-day)",
           ylabel="share of replays that detect it")
    c.hline(0.8, ORANGE)
    c.vline(mde, ORANGE)
    c.line(curve, TEAL, 2.8)
    c.text(mde + 0.4, 0.42, f"minimum detectable effect ≈ ${mde:.1f}", size=12, color=ORANGE, weight=600)
    c.text(0.3, 0.83, "80% power", size=11, color=ORANGE)
    c.text(0.3, curve[0][1] + 0.04, f"{curve[0][1] * 100:.0f}% false alarms at $0", size=11)
    save("placebo-power.svg", c)
    return mde, curve[0][1]


# 10. Compliance dilution -------------------------------------------------
def compliance():
    c = Chart(760, 260, 0, 15, 0, 20, top=36, bottom=40,
              title="Missed switches water the effect down")
    c.grid([0, 5, 10, 15, 20], fmt=lambda v: f"${v:g}")
    for i in range(10):
        landed = i < 7
        c.rect(i + 0.15, i + 0.85, 0, 15 if landed else 0.25, TEAL if landed else RED, 0.85)
        c.text(i + 0.5, -2.2, "landed" if landed else "missed", size=10, anchor="middle",
               color=TEXT if landed else RED)
    c.hline(10.5, ORANGE)
    c.text(10.15, 11.2, "measured average", size=12, color=ORANGE, weight=600)
    c.text(10.15, 9.0, "≈ $10.50 (70% of $15)", size=12, color=ORANGE)
    c.text(10.15, 15.4, "the promo's own", size=11)
    c.text(10.15, 13.8, "effect: $15", size=11)
    c.hline(15, TEAL, dash="2 3", width=1)
    save("compliance-dilution.svg", c)


# 11. Carryover patterns ---------------------------------------------------
def carryover():
    c = Chart(760, 230, 0, 33, -12, 26, top=40, bottom=30, title="After the lever turns off: three possible readings")
    shapes = [("Lingered", TEAL, lambda t: 20 if t < 4 else 20 * math.exp(-(t - 4) / 2.2)),
              ("None measurable", SLATE, lambda t: 20 if t < 4 else 0),
              ("Pull-forward", RED, lambda t: 20 if t < 4 else -8 * math.exp(-(t - 4) / 2.5))]
    for k, (name, col, f) in enumerate(shapes):
        x0 = k * 11
        c.span(x0 + 4.2, x0 + 10.5, col, 0.08)
        c.line([(x0 + 0.5 + t * 0.95, f(t)) for t in [i / 4 for i in range(41)]], col, 2.6)
        c.line([(x0 + 0.5, 0), (x0 + 10.5, 0)], TEXT, 1, dash="2 3")
        c.text(x0 + 5.5, 24, name, size=13, anchor="middle", color=col, weight=600)
        c.text(x0 + 2.4, -10, "lever on", size=10, anchor="middle")
        c.text(x0 + 10.4, 1.2, "no effect", size=9, anchor="end")
        c.text(x0 + 7.4, -10, "after it turned off", size=10, anchor="middle")
    save("carryover-patterns.svg", c)


if __name__ == "__main__":
    rollout_series()
    eff = unit_changes()
    obs, p = permutation()
    local_shock()
    diffs = switchback()
    hero, sib, spend, attr = geo_lift()
    predicted(hero, spend)
    matched_pairs()
    mde, fa = placebo_power()
    compliance()
    carryover()
    json.dump({"wave1_effect": eff, "rollout_att": obs, "rollout_p": p, "switchback_diffs": diffs,
               "hero": hero, "sib": sib, "spend": spend, "attr": attr, "sim_mde": mde, "sim_false_alarm": fa},
              sys.stdout, indent=1)
