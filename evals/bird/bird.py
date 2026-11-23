#!/usr/bin/env python3
"""BIRD Mini-Dev data CLI: fetch | load | smoke | gen-cases | evidence | report."""
import argparse
import sys
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))

from birdlib import config  # noqa: E402
from birdlib.cases import HALVES, RUNTIMES  # noqa: E402
from birdlib import fetch as fetch_mod  # noqa: E402


def main(argv: list[str] | None = None) -> int:
    p = argparse.ArgumentParser(prog="bird.py", description=__doc__)
    sub = p.add_subparsers(dest="cmd", required=True)
    sub.add_parser("fetch", help="download the zip and question jsonl into the cache")
    load = sub.add_parser("load", help="split the dump and load the 4 DBs into Docker Postgres")
    load.add_argument("--fresh", action="store_true", help="drop and re-create existing DBs")
    sub.add_parser("smoke", help="check row counts, read-only role, and gold SQL")
    gen = sub.add_parser("gen-cases", help="write <db>.test.yml files from the cached questions")
    gen.add_argument("--runs", type=int, default=1, help="runs per case (default 1)")
    gen.add_argument("--runtime", choices=RUNTIMES, default="fsm",
                     help="react writes <db>.react.test.yml against <db>.react.agentic.yml; "
                     "semantic (react with semantic_only) writes <db>.semantic.test.yml")
    gen.add_argument("--half", choices=HALVES, default=None,
                     help="only the first (a) or second (b) half of each db's questions by id, "
                     "written to <db>[.<runtime>].half-<a|b>.test.yml (memory transfer pass)")
    rp = sub.add_parser("report", help="render the report from <db>[.react].test.results.json")
    rp.add_argument("--runtime", choices=RUNTIMES, default="fsm")
    rp.add_argument("--dir", default=None, help="results directory (default evals/bird)")
    cp = sub.add_parser("compare", help="FSM vs react tables from two results directories")
    cp.add_argument("--fsm-dir", required=True)
    cp.add_argument("--react-dir", required=True)
    cp.add_argument("--react-arm", choices=("react", "semantic"), default="react",
                    help="which react arm's results --react-dir holds")
    for q in (rp, cp):
        q.add_argument("--model", required=True)
        q.add_argument("--price-in", type=float, required=True, help="$ per 1M input tokens")
        q.add_argument("--price-out", type=float, required=True, help="$ per 1M output tokens")
        q.add_argument("--price-cache-read", type=float, default=None,
                       help="$ per 1M cache-read tokens (default 0.1x price-in)")
        q.add_argument("--price-cache-write", type=float, default=None,
                       help="$ per 1M cache-write tokens (default 1.25x price-in, 5-minute TTL)")
    ev = sub.add_parser("evidence", help="print a db's evidence strings and column descriptions")
    ev.add_argument("db", choices=config.DBS)
    args = p.parse_args(argv)
    try:
        return _run(args)
    except ValueError as e:  # a refusal: mismatched models, runtimes or arms
        print(f"error: {e}", file=sys.stderr)
        return 2


def _run(args: argparse.Namespace) -> int:
    if args.cmd == "fetch":
        fetch_mod.fetch()
    elif args.cmd == "load":
        from birdlib import load as load_mod
        load_mod.load(fresh=args.fresh)
    elif args.cmd == "gen-cases":
        from birdlib import cases
        cases.gen_cases(runs=args.runs, runtime=args.runtime, half=args.half)
    elif args.cmd == "report":
        from birdlib import report
        results = report.load_results(args.runtime, Path(args.dir) if args.dir else report.ROOT)
        sys.stdout.write(report.render(results, args.model, args.price_in, args.price_out,
                                       args.price_cache_read, args.price_cache_write,
                                       args.runtime))
    elif args.cmd == "compare":
        from birdlib import compare
        sys.stdout.write(compare.run(args.fsm_dir, args.react_dir, args.model, args.price_in,
                                     args.price_out, args.price_cache_read,
                                     args.price_cache_write, react_arm=args.react_arm))
    elif args.cmd == "evidence":
        from birdlib import evidence
        evidence.print_evidence(args.db)
    else:
        from birdlib import load as load_mod
        return load_mod.smoke()
    return 0


if __name__ == "__main__":
    sys.exit(main())
