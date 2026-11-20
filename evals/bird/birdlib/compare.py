"""FSM vs react comparison: one table per db and pooled, plus the questions whose verdicts differ."""
from pathlib import Path

from . import report, settings

ROWS = (
    ("answer accuracy", lambda s: f"{report._pct(s['answer_acc'])} ({s['correct']}/{s['judged']})"),
    ("EX", lambda s: report._pct(s["ex"])),
    ("Soft-F1", lambda s: report._pct(s["soft_f1"])),
    ("median latency s", lambda s: f"{s['lat_median']:.1f}"),
    ("median turns", lambda s: f"{s['rounds_median']:.1f}"),
    ("tokens in / out", lambda s: f"{s['input_tokens']:,} / {s['output_tokens']:,}"),
    ("cost", lambda s: f"${s['cost']:.2f}"),
)


def _table(label: str, fsm: dict, react: dict) -> list[str]:
    lines = [f"### {label}", "", "| metric | fsm | react |", "|---|---|---|"]
    lines += [f"| {name} | {fmt(fsm)} | {fmt(react)} |" for name, fmt in ROWS]
    return lines + [""]


def _verdicts(results: dict) -> dict:
    """(db, question_id) -> sorted verdicts of its graded records (SKIP excluded)."""
    out: dict = {}
    for db, data in results.items():
        for r in report._execution(data)["records"]:
            if r["choice"] == "SKIP" or "question_id" not in r:
                continue
            out.setdefault((db, r["question_id"]), []).append(r.get("judge_verdict") or "?")
    return {k: sorted(v) for k, v in out.items()}


def untagged_count(results: dict) -> int:
    return sum(1 for data in results.values() for r in report._execution(data)["records"]
               if r["choice"] != "SKIP" and "question_id" not in r)


def differing(fsm_results: dict, react_results: dict) -> list[tuple]:
    """`(db, question_id, fsm verdicts, react verdicts)` where the two arms disagree. A question
    graded in only one arm counts as differing (its other side is empty)."""
    a, b = _verdicts(fsm_results), _verdicts(react_results)
    return [(db, q, a.get((db, q), []), b.get((db, q), []))
            for db, q in sorted(set(a) | set(b)) if a.get((db, q), []) != b.get((db, q), [])]


def check_same_model(fsm_results: dict, react_results: dict) -> None:
    """Both arms must run the same model. Records from before the field (the archived baseline)
    are `unrecorded` and cannot be checked; that is printed, not refused."""
    a, b = settings.recorded_models(fsm_results), settings.recorded_models(react_results)
    if settings.UNRECORDED in a + b:
        return
    if a != b:
        raise ValueError(f"the arms ran different models: fsm {a}, react {b}; a comparison "
                         "needs the same model on both sides")


def _judges(results: dict) -> str:
    found = sorted({report._execution(r)["judge_model"] for r in results.values()} - {None})
    return ", ".join(found) or "unknown"


def render(fsm_results: dict, react_results: dict, model: str, price_in: float,
           price_out: float, price_cache_read=None, price_cache_write=None) -> str:
    settings.check_declared(fsm_results, model, "fsm")
    settings.check_declared(react_results, model, "react")
    check_same_model(fsm_results, react_results)
    args = (price_in, price_out, price_cache_read, price_cache_write)
    fsm, react = report.summarize(fsm_results, *args), report.summarize(react_results, *args)
    lines = ["# BIRD Mini-Dev: FSM vs react runtime", "",
             f"Eval model: `{model}` for both arms (${price_in}/M input, ${price_out}/M output). "
             f"Judge model: `{_judges(fsm_results)}` (fsm), `{_judges(react_results)}` (react).",
             "", "- fsm: " + settings.describe(fsm_results, "fsm"),
             "- react: " + settings.describe(react_results, "react"), "",
             "Runs that failed before answering carry no usage: they are excluded from the median "
             "latency and turns, and their tokens and cost are missing, so cost is a lower bound "
             "on that count.", "",
             "Median turns is the median LLM calls per question (one `LlmStart` each) for both "
             "runtimes, over runs that produced a measurement. Token counts read the same event "
             "fields for both arms. Output and cache tokens are the provider's in both. Input "
             "tokens are the provider's uncached input for react; the FSM reports its prompt "
             "size on `LlmStart`: an estimate on the first round, then the previous round's provider "
             "uncached input plus its output plus an estimate for the tool results, so input is "
             "close but not identical in meaning.", ""]
    for db in fsm_results:
        lines += _table(db, fsm[db], react[db])
    lines += _table("pooled", fsm["total"], react["total"])
    diff = differing(fsm_results, react_results)
    untagged = untagged_count(fsm_results) + untagged_count(react_results)
    if untagged:
        lines += [f"{untagged} graded records carry no question id (prompt not unique) and are "
                  "left out of the verdict list.", ""]
    lines += [f"## Questions with differing verdicts ({len(diff)})", ""]
    lines += [f"- {db} {q}: fsm {'/'.join(f) or 'none'}, react {'/'.join(r) or 'none'}"
              for db, q, f, r in diff] or ["(none)"]
    return "\n".join(lines) + "\n"


def run(fsm_dir, react_dir, *args) -> str:
    fsm = report.load_results("fsm", Path(fsm_dir))
    react = report.load_results("react", Path(react_dir))
    return render(fsm, react, *args)
