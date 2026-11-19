#!/usr/bin/env python3
"""BIRD Mini-Dev data CLI: fetch | load | smoke | gen-cases | evidence | report."""
import argparse
import sys
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))

from birdlib import config  # noqa: E402
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
    rp = sub.add_parser("report", help="render the baseline report from <db>.test.results.json")
    rp.add_argument("--model", required=True)
    rp.add_argument("--price-in", type=float, required=True, help="$ per 1M input tokens")
    rp.add_argument("--price-out", type=float, required=True, help="$ per 1M output tokens")
    rp.add_argument("--price-cache-read", type=float, default=None,
                    help="$ per 1M cache-read tokens (default 0.1x price-in)")
    rp.add_argument("--price-cache-write", type=float, default=None,
                    help="$ per 1M cache-write tokens (default 1.25x price-in, 5-minute TTL)")
    ev = sub.add_parser("evidence", help="print a db's evidence strings and column descriptions")
    ev.add_argument("db", choices=config.DBS)
    args = p.parse_args(argv)

    if args.cmd == "fetch":
        fetch_mod.fetch()
    elif args.cmd == "load":
        from birdlib import load as load_mod
        load_mod.load(fresh=args.fresh)
    elif args.cmd == "gen-cases":
        from birdlib import cases
        cases.gen_cases(runs=args.runs)
    elif args.cmd == "report":
        from birdlib import report
        sys.stdout.write(report.render(report.load_results(), args.model,
                                       args.price_in, args.price_out,
                                       args.price_cache_read, args.price_cache_write))
    elif args.cmd == "evidence":
        from birdlib import evidence
        evidence.print_evidence(args.db)
    else:
        from birdlib import load as load_mod
        return load_mod.smoke()
    return 0


if __name__ == "__main__":
    sys.exit(main())
