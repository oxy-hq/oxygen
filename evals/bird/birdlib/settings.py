"""Model and thinking settings behind a results set, read from the data and the agent files."""
import re
from pathlib import Path

from . import cases, report

UNRECORDED = "unrecorded"


def recorded_models(results: dict) -> list[str]:
    """Models the records report (answered runs only; a failed run never saw a model). Records
    from before the field exist are `unrecorded`."""
    found = set()
    for data in results.values():
        for r in report._execution(data)["records"]:
            if r["choice"] == "SKIP" or report.run_failed(r):
                continue
            found.add(r.get("model") or UNRECORDED)
    return sorted(found)


def check_declared(results: dict, model: str, arm: str) -> None:
    """Refuse when any recorded model differs from the declared `--model`. An `unrecorded` model
    (the archived FSM records) cannot be checked and passes on the fsm arm only: every react
    record is written with its model, so a react record without one is refused."""
    models = recorded_models(results)
    if arm != "fsm" and UNRECORDED in models:
        raise ValueError(f"{arm}: records report model {UNRECORDED!r}; react records always "
                         "carry a model, so this file is not a react run's")
    bad = [m for m in models if m not in (UNRECORDED, model)]
    if bad:
        raise ValueError(f"{arm}: records report model {bad} but --model is {model!r}")


def thinking_settings(runtime: str, root: Path = report.ROOT) -> str:
    """The `thinking:` keys in the runtime's agent files and `config.yml`; none set means the
    solving state's default (adaptive)."""
    files = [cases.agent_file(db, runtime) for db in report.config.DBS] + ["config.yml"]
    lines = set()
    for name in files:
        for line in (Path(root) / name).read_text().splitlines():
            if re.match(r"\s*thinking\s*:", line):
                lines.add(line.strip())
    got = ", ".join(sorted(lines)) if lines else "default (adaptive): no `thinking:` key set"
    # The FSM's per-state `states.<state>.thinking` keys also match; the default is for the
    # solving state, the one that answers.
    return f"solving state: {got} (from the agent files in this checkout, not from the run)"


def describe(results: dict, runtime: str, root: Path = report.ROOT) -> str:
    return (f"Recorded model: `{', '.join(recorded_models(results)) or UNRECORDED}`. "
            f"Thinking: {thinking_settings(runtime, root)}.")
