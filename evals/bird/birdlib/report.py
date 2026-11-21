"""Render the FSM baseline report from `oxy test --output-json` results files."""
import json
import statistics
from pathlib import Path

from . import cases, config

ROOT = Path(__file__).resolve().parent.parent


def _execution(result_list) -> dict:
    """Merge every Execution metric in an EvalResult list into one record bag."""
    records, judge_model = [], None
    for res in result_list:
        for m in res.get("metrics", []):
            if m.get("type") == "Execution":
                records += m.get("records", [])
                judge_model = judge_model or m.get("judge_model")
    return {"records": records, "judge_model": judge_model}


def run_failed(r: dict) -> bool:
    """A run that errored before answering: `failed_run_record` gives it no answer, no timing and
    zero duration, tokens and rounds. It counts in accuracy but has no measurement."""
    return not r.get("answer_text") and "Run failed" in (r.get("cot") or "")


def _stats(records: list[dict], price_in: float, price_out: float, price_cr: float,
           price_cw: float) -> dict:
    graded = [r for r in records if r["choice"] != "SKIP"]
    n = len(graded)
    measured = [r for r in graded if not run_failed(r)]
    m = len(measured)
    verdicts = [r.get("judge_verdict") for r in graded]
    judge_errors = verdicts.count("JUDGE_ERROR")
    judged = n - judge_errors
    correct = verdicts.count("CORRECT")
    secs = [r["duration_ms"] / 1000 for r in measured]
    tin = sum(r["input_tokens"] for r in graded)
    tout = sum(r["output_tokens"] for r in graded)
    cr = sum((r.get("timing") or {}).get("cache_read_input_tokens", 0) for r in graded)
    cw = sum((r.get("timing") or {}).get("cache_creation_input_tokens", 0) for r in graded)
    div = lambda a, b: a / b if b else 0.0
    return {
        "graded": n, "skipped": len(records) - n, "judged": judged, "correct": correct,
        "judge_errors": judge_errors, "measured": m, "run_failures": n - m,
        "answer_acc": div(correct, judged),
        # `score` is the per-record EX bit (1.0 = result sets match), in old and new results files.
        "ex": div(sum(r["score"] for r in graded), n),
        "soft_f1": div(sum(r.get("soft_f1") or 0.0 for r in graded), n),
        # Latency and rounds average only runs with a measurement (run failures carry zeros).
        "lat_mean": div(sum(secs), m), "lat_median": statistics.median(secs) if secs else 0.0,
        "rounds": div(sum(r.get("llm_rounds", 0) for r in measured), m),
        # Turns = LLM calls (one `LlmStart` each) for both runtimes; see README "Turns and tokens".
        "rounds_median": statistics.median([r.get("llm_rounds", 0) for r in measured])
        if measured else 0.0,
        "input_tokens": tin, "output_tokens": tout,
        "cache_read_tokens": cr, "cache_write_tokens": cw,
        "cost": (tin * price_in + tout * price_out + cr * price_cr + cw * price_cw) / 1e6,
        "correct_ex_fail": sum(1 for r in graded
                               if r.get("judge_verdict") == "CORRECT" and r["score"] < 1.0),
    }


def cache_prices(price_in: float, price_cache_read=None, price_cache_write=None) -> tuple:
    """Defaults: cache read 0.1x and 5-minute cache write 1.25x of the input price (the agent
    sends `cache_control: {"type": "ephemeral"}` with no ttl, so 5-minute)."""
    return (0.1 * price_in if price_cache_read is None else price_cache_read,
            1.25 * price_in if price_cache_write is None else price_cache_write)


def summarize(results: dict, price_in: float = 0.0, price_out: float = 0.0,
              price_cache_read=None, price_cache_write=None) -> dict:
    """Per-db stats plus a `total` over the pooled records (weights by graded/judged count)."""
    per = {db: _execution(res) for db, res in results.items()}
    cr, cw = cache_prices(price_in, price_cache_read, price_cache_write)
    out = {db: _stats(e["records"], price_in, price_out, cr, cw) for db, e in per.items()}
    pooled = [r for e in per.values() for r in e["records"]]
    out["total"] = _stats(pooled, price_in, price_out, cr, cw)
    return out


def _pct(x: float) -> str:
    return f"{x * 100:.1f}%"


def _row(label: str, s: dict) -> str:
    return (f"| {label} | {_pct(s['answer_acc'])} ({s['correct']}/{s['judged']}) | {s['judge_errors']} "
            f"| {_pct(s['ex'])} | {_pct(s['soft_f1'])} | {s['graded']} | {s['skipped']} "
            f"| {s['measured']} | {s['lat_mean']:.1f} / {s['lat_median']:.1f} | {s['rounds']:.1f} "
            f"| {s['input_tokens']:,} / {s['cache_read_tokens']:,} / {s['cache_write_tokens']:,} "
            f"/ {s['output_tokens']:,} | ${s['cost']:.2f} |")


def _one_line(s: str | None, n: int = 200) -> str:
    s = " ".join((s or "").split())
    return s[:n] + ("..." if len(s) > n else "")


def _id(r: dict) -> str:
    return str(r.get("question_id", "?"))


def failure_kind(cot: str) -> str:
    """Bucket a failed run's `cot` ("Run failed with error: ...") into a short label."""
    c = cot or ""
    if "asked a clarifying question" in c:
        return "agent asked a clarifying question"
    if "max iterations" in c:
        return "max iterations exceeded"
    if "SyntaxError" in c or "syntax" in c.lower():
        return "generated SQL syntax error"
    return "other run error"


TITLES = {"fsm": "BIRD Mini-Dev FSM baseline", "react": "BIRD Mini-Dev react runtime"}


def render(results: dict, model: str, price_in: float, price_out: float,
           price_cache_read=None, price_cache_write=None, runtime: str = "fsm") -> str:
    from . import settings  # settings imports this module
    settings.check_declared(results, model, runtime)
    stats = summarize(results, price_in, price_out, price_cache_read, price_cache_write)
    pcr, pcw = cache_prices(price_in, price_cache_read, price_cache_write)
    judges = sorted({_execution(r)["judge_model"] for r in results.values()} - {None})
    t = stats["total"]
    lines = [
        f"# {TITLES[runtime]}",
        "",
        f"Eval model: `{model}` (${price_in}/M input, ${price_out}/M output, "
        f"${pcr:g}/M cache read, ${pcw:g}/M cache write). "
        f"Judge model: `{', '.join(judges) or 'unknown'}`.",
        "",
        settings.describe(results, runtime),
        "",
        f"**Answer accuracy: {_pct(t['answer_acc'])} ({t['correct']}/{t['judged']} judged)**; "
        f"EX {_pct(t['ex'])}, Soft-F1 {_pct(t['soft_f1'])}; {t['graded']} graded, "
        f"{t['skipped']} skipped, {t['judge_errors']} judge errors; ${t['cost']:.2f} agent cost.",
        "",
        f"Context: {t['correct_ex_fail']} of {t['correct']} judge-CORRECT answers had EX FAIL "
        "(how much strict result-set EX under-reads the FSM).",
        "",
        "Latency and LLM rounds average the measured runs only (n per row); run failures carry "
        "no duration, rounds or tokens.",
        "",
        "| db | answer acc (correct/judged) | judge errors | EX | Soft-F1 | graded | skipped "
        "| measured n | latency s mean / median | LLM rounds | tokens uncached in / cache read / "
        "cache write / out | $ |",
        "|---|---|---|---|---|---|---|---|---|---|---|---|",
    ]
    lines += [_row(db, stats[db]) for db in results] + [_row("**total**", t)]
    recs = [(db, r) for db in results for r in _execution(results[db])["records"]]
    sections = [
        ("Skipped (gold error)", lambda r: r["choice"] == "SKIP", "cot"),
        ("Failures (judge INCORRECT)", lambda r: r.get("judge_verdict") == "INCORRECT",
         "judge_reason"),
        ("Judge errors", lambda r: r.get("judge_verdict") == "JUDGE_ERROR", "judge_reason"),
    ]
    for title, pred, field in sections:
        lines += ["", f"## {title}", ""]
        hits = [(db, r) for db, r in recs if pred(r)]
        no_answer = lambda r: not r.get("answer_text")
        lines += [f"- {db} {_id(r)}: " + _one_line(
            r.get("cot") if (field == "judge_reason" and no_answer(r)) else (r.get(field) or r.get("cot")))
            for db, r in hits] or ["(none)"]
    failed = [r for _, r in recs if r["choice"] != "SKIP" and run_failed(r)]
    kinds: dict = {}
    for r in failed:
        kinds[failure_kind(r["cot"])] = kinds.get(failure_kind(r["cot"]), 0) + 1
    lines += ["", "## Run failures (no answer; judged INCORRECT)", "",
              f"{len(failed)} runs ended without an answer: "
              + (", ".join(f"{n} {k}" for k, n in sorted(kinds.items(), key=lambda kv: -kv[1]))
                 or "none") + ". They count against answer accuracy and carry no timing or usage."]
    lines += render_sources([r for _, r in recs])
    lines += render_timing({db: _execution(results[db])["records"] for db in results},
                           [r for _, r in recs], runtime)
    lines += ["", f"Judge-call tokens are not in the agent token counts or the $ column. "
              f"{t['run_failures']} run failures carry no usage; their entire cost is missing, so $ "
              "is a lower bound."]
    return "\n".join(lines) + "\n"


COMPONENTS = ("llm", "tool", "query", "startup", "other")

NOT_RECORDED = "not recorded"


def source_summary(records: list[dict]) -> dict:
    """Graded answers by the source of the SQL they cite (`timing.answer_source`), with judge
    accuracy each, and executions by source (`timing.queries_by_source`)."""
    answers: dict = {}
    executions: dict = {}
    for r in records:
        if r["choice"] == "SKIP":
            continue
        t = r.get("timing") or {}
        a = answers.setdefault(t.get("answer_source") or NOT_RECORDED,
                               {"answers": 0, "judged": 0, "correct": 0})
        a["answers"] += 1
        a["judged"] += r.get("judge_verdict") != "JUDGE_ERROR"
        a["correct"] += r.get("judge_verdict") == "CORRECT"
        for src, n in (t.get("queries_by_source") or {}).items():
            executions[src] = executions.get(src, 0) + n
    return {"answers": answers, "executions": executions}


def render_sources(records: list[dict]) -> list[str]:
    s = source_summary(records)
    if set(s["answers"]) <= {NOT_RECORDED}:
        return ["", "## Query source", "",
                "Not recorded: these results predate `answer_source` / `queries_by_source`."]
    div = lambda a, b: a / b if b else 0.0
    order = sorted(s["answers"], key=lambda k: (k == NOT_RECORDED, -s["answers"][k]["answers"]))
    lines = ["", "## Query source", "",
             "Answers by the source of the SQL they cite: `semantic` is SQL the semantic model "
             "compiled, `llm` is SQL the model wrote. `not recorded` covers runs with no final "
             "SQL (run failures, answers citing no result) and, for react, an answer whose cited "
             "SQL matched no execution.", "",
             "| source | answers | correct / judged | answer acc |", "|---|---|---|---|"]
    for k in order:
        a = s["answers"][k]
        lines.append(f"| {k} | {a['answers']} | {a['correct']}/{a['judged']} | "
                     f"{_pct(div(a['correct'], a['judged']))} |")
    ex = s["executions"]
    lines += ["", "Executions by source (every query run, failures included): "
              + (", ".join(f"{k} {n}" for k, n in sorted(ex.items(), key=lambda kv: -kv[1]))
                 or "none") + "."]
    return lines


def _pctile(xs: list[float], q: float) -> float:
    if not xs:
        return 0.0
    xs = sorted(xs)
    return xs[min(len(xs) - 1, max(0, int(round(q * (len(xs) - 1)))))]


def _dominant(t: dict) -> str:
    parts = {"llm": t["llm_ms"], "tool": t["tool_ms"], "query": t["query_ms"],
             "startup": t["startup_ms"], "other": t.get("other_ms") or 0}
    return max(parts, key=parts.get)


def timing_summary(records: list[dict]) -> dict:
    """Aggregate per-record `timing` objects. Records without one (errored runs) are counted
    in `untimed` but never timed. `other` is summed over non-fanout runs only."""
    graded = [r for r in records if r["choice"] != "SKIP"]
    timed = [r for r in graded if r.get("timing")]
    T = [r["timing"] for r in timed]
    wall = sum(t["wall_ms"] for t in T)
    div = lambda a, b: a / b if b else 0.0
    nf = [t for t in T if not t.get("fanout") and t.get("other_ms") is not None]
    sums = {"llm": sum(t["llm_ms"] for t in T), "tool": sum(t["tool_ms"] for t in T),
            "query": sum(t["query_ms"] for t in T), "startup": sum(t["startup_ms"] for t in T)}
    share = {k: div(v, wall) for k, v in sums.items()}
    share["other"] = div(sum(t["other_ms"] for t in nf), sum(t["wall_ms"] for t in nf))
    calls = sum(t["llm_calls"] for t in T)
    ttft_rounds = sum(t["llm_ttft_rounds"] for t in T)
    cache_den = sum(t["cache_read_input_tokens"] + t["cache_creation_input_tokens"]
                    for t in T) + sum(r["input_tokens"] for r in timed)

    def merge(key: str, field: str) -> dict:
        out: dict = {}
        for t in T:
            for name, v in t[key].items():
                e = out.setdefault(name, {"calls": 0, "ms": 0, "errors": 0})
                if isinstance(v, dict):
                    e["calls"] += v.get("calls", 0)
                    e["ms"] += v.get(field, 0)
                    e["errors"] += v.get("errors", 0)
                else:
                    e["ms"] += v
        return out

    slowest = sorted(timed, key=lambda r: -r["timing"]["wall_ms"])[:5]
    return {
        "graded": len(graded), "timed": len(T), "untimed": len(graded) - len(T),
        "wall_median_s": _pctile([t["wall_ms"] / 1000 for t in T], 0.5) if T else 0.0,
        "wall_p90_s": _pctile([t["wall_ms"] / 1000 for t in T], 0.9),
        "wall_total_s": wall / 1000, "fanout_runs": sum(1 for t in T if t.get("fanout")),
        "other_runs": len(nf), "share": share, "sums_ms": sums,
        "calls_mean": div(calls, len(T)), "ms_per_call": div(sums["llm"], calls),
        "ttft_coverage": (ttft_rounds, calls),
        "ttft_mean_ms": div(sum(t["llm_ttft_ms"] for t in T), ttft_rounds),
        "stream_mean_ms": div(sum(t["llm_stream_ms"] for t in T), ttft_rounds),
        "out_tokens_mean": div(sum(t["output_tokens"] for t in T), len(T)),
        "cache_read_share": div(sum(t["cache_read_input_tokens"] for t in T), cache_den),
        "llm_states": merge("llm_by_state", "ms"), "states": merge("states", "ms"),
        "tools": merge("tools_by_name", "ms"),
        "slowest": [{"question_id": r.get("question_id", "?"), "wall_s": r["timing"]["wall_ms"] / 1000,
                     "dominant": _dominant(r["timing"]),
                     "dominant_ms": {"llm": r["timing"]["llm_ms"], "tool": r["timing"]["tool_ms"],
                                     "query": r["timing"]["query_ms"],
                                     "startup": r["timing"]["startup_ms"],
                                     "other": r["timing"].get("other_ms") or 0}[_dominant(r["timing"])]}
                    for r in slowest],
    }


def _top(d: dict, n: int = 5, errors: bool = False) -> str:
    items = sorted(d.items(), key=lambda kv: -kv[1]["ms"])[:n]
    return ", ".join(f"{k} {v['ms'] / 1000:.1f}s" + (f" ({v['calls']} calls, {v['errors']} err)"
                     if errors else "") for k, v in items) or "n/a"


def _timing_row(label: str, s: dict) -> str:
    sh = s["share"]
    return (f"| {label} | {s['timed']} / {s['untimed']} | {s['wall_median_s']:.1f} | "
            f"{s['wall_p90_s']:.1f} | {_pct(sh['llm'])} | {_pct(sh['tool'])} | {_pct(sh['query'])} | "
            f"{_pct(sh['startup'])} | {_pct(sh['other'])} ({s['other_runs']} runs) |")


def render_timing(per_db: dict, pooled: list[dict], runtime: str = "fsm") -> list[str]:
    tot = timing_summary(pooled)
    lines = ["", "## Timing", "",
             "Wall time per question; shares are of summed wall time across timed runs. LLM, tool, "
             "query and startup shares cover all timed runs; `other` (wall - startup - llm - tool - "
             "query) is null under fan-out, so it is computed over non-fanout runs only (run count "
             "shown). Errored runs carry no timing: counted, not timed. `startup` and `states` are "
             "receipt-timed and can lag ~100 ms: use them for ranking.", "",
             "| db | timed / untimed | wall median s | wall p90 s | LLM | tool | query | startup | other |",
             "|---|---|---|---|---|---|---|---|---|"]
    lines += [_timing_row(db, timing_summary(rs)) for db, rs in per_db.items()]
    lines.append(_timing_row("**total**", tot))
    c = tot["ttft_coverage"]
    na = f"n/a ({runtime})" if runtime != "fsm" else "n/a"
    ttft = f"{tot['ttft_mean_ms']:.0f} ms" if c[0] else na
    stream = f"{tot['stream_mean_ms']:.0f} ms" if c[0] else na
    lines += ["", "### LLM detail (all dbs)", "",
              f"- calls per question {tot['calls_mean']:.1f}; mean {tot['ms_per_call']:.0f} ms per call",
              f"- TTFT mean {ttft} and stream mean {stream} "
              f"per measured round; coverage {c[0]} / {c[1]} rounds (llm_ttft_rounds / llm_calls). "
              "Schema rounds and tool-call-only rounds stream no token and are excluded, so "
              "ttft + stream is below llm_ms. TTFT is a proxy for network + queueing + prefill and "
              "cannot be split further.",
              f"- output tokens per question {tot['out_tokens_mean']:.0f}; cache-read share of input "
              f"{_pct(tot['cache_read_share'])} (cache_read / (cache_read + cache_creation + uncached input))",
              "", "### Slowest components (summed over all timed runs)", "",
              f"- LLM by state: {_top(tot['llm_states'])}",
              f"- states (receipt-timed): {_top(tot['states'])}",
              f"- tools: {_top(tot['tools'], errors=True)}",
              "", "### 5 slowest questions", ""]
    lines += [f"- {s['question_id']}: {s['wall_s']:.1f}s, dominant {s['dominant']} "
              f"({s['dominant_ms'] / 1000:.1f}s)" for s in tot["slowest"]] or ["(none)"]
    lines += ["", "## Bottleneck", ""]
    if not tot["timed"]:
        return lines + ["No timed runs."]
    sh = tot["share"]
    top = max(("llm", "tool", "query", "startup", "other"), key=lambda k: sh[k])
    lines.append(
        f"Median question takes {tot['wall_median_s']:.1f}s (p90 {tot['wall_p90_s']:.1f}s). "
        f"{top} is the largest share at {_pct(sh[top])} of wall time (LLM {_pct(sh['llm'])}, tool "
        f"{_pct(sh['tool'])}, query {_pct(sh['query'])}, startup {_pct(sh['startup'])}, other "
        f"{_pct(sh['other'])} over {tot['other_runs']} non-fanout runs; {tot['fanout_runs']} fan-out "
        f"runs). The LLM runs {tot['calls_mean']:.1f} rounds per question at "
        f"{tot['ms_per_call'] / 1000:.1f}s each. Slowest LLM state: {_top(tot['llm_states'], 1)}.")
    return lines


def check_runtime(records: list[dict], runtime: str, where: str) -> None:
    """Raise when a record names another runtime. A record without the field is `fsm` (the
    archived baseline). A run that failed before answering never recorded one, so it is exempt."""
    answered = [r for r in records if not run_failed(r)]
    for r in answered:
        got = r.get("runtime") or "fsm"
        if got != runtime:
            raise ValueError(f"{where}: record runtime {got!r} is not the requested {runtime!r}")
    if runtime == "react" and records and not any(r.get("runtime") == "react" for r in answered):
        raise ValueError(f"{where}: no record is labeled react (every run failed?)")


def load_results(runtime: str = "fsm", root: Path = ROOT, dbs=config.DBS) -> dict:
    """Read `<db>.test.results.json` (fsm) or `<db>.react.test.results.json` (react) from `root`
    and tag each record with its question id (by prompt)."""
    if runtime not in cases.RUNTIMES:
        raise ValueError(f"unknown runtime {runtime!r}")
    ids: dict[str, list[int]] = {}
    for row in cases.load_rows():
        ids.setdefault(row["question"], []).append(row["question_id"])
    out = {}
    for db in dbs:
        path = Path(root) / cases.test_file(db, runtime).replace(".test.yml", ".test.results.json")
        data = json.loads(path.read_text())
        records = _execution(data)["records"]
        check_runtime(records, runtime, path.name)
        for r in records:
            c = ids.get(r.get("prompt") or "", [])
            if len(c) == 1:
                r["question_id"] = c[0]
        out[db] = data
    return out
