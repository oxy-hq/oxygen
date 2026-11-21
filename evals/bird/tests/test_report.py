import json
import tempfile
import unittest
from pathlib import Path
from unittest import mock

from birdlib import compare, report


def rec(choice="PASS", verdict="CORRECT", qid=1, ms=1000.0, tin=100, tout=10, f1=1.0, rounds=2,
        reason="ok"):
    return {"choice": choice, "score": 1.0 if choice == "PASS" else 0.0, "soft_f1": f1,
            "duration_ms": ms, "input_tokens": tin, "output_tokens": tout, "llm_rounds": rounds,
            "judge_verdict": verdict, "judge_reason": reason, "question_id": qid,
            "cot": f"cot {qid}"}


def results(records):
    return [{"metrics": [{"type": "Execution", "judge_model": "judge-x", "records": records}]}]


def timing(wall=10000, startup=500, llm=6000, calls=4, tool=1000, query=2000, fanout=False,
           ttft_rounds=2, ttft=800, stream=3000, out=400, cr=900, cc=100, other="auto",
           states=None, by_state=None, tools=None):
    o = max(0, wall - startup - llm - tool - query)
    return {"wall_ms": wall, "startup_ms": startup, "llm_calls": calls, "llm_ms": llm,
            "llm_ttft_rounds": ttft_rounds, "llm_ttft_ms": ttft, "llm_stream_ms": stream,
            "output_tokens": out, "cache_read_input_tokens": cr, "cache_creation_input_tokens": cc,
            "llm_by_state": by_state or {"solve": {"calls": calls, "ms": llm}},
            "tool_calls": 2, "tool_ms": tool,
            "tools_by_name": tools or {"run_sql": {"calls": 2, "ms": tool, "errors": 1}},
            "query_count": 2, "query_ms": query, "query_failures": 0,
            "states": states or {"solve": 7000}, "fanout": fanout,
            "other_ms": None if fanout else (o if other == "auto" else other)}


class CacheCostTests(unittest.TestCase):
    def test_cache_tokens_priced_and_untimed_disclosed(self):
        a = rec(qid=1, tin=1000, tout=100)
        a["timing"] = timing(cr=2_000_000, cc=1_000_000)
        b = rec(qid=2, tin=1000, tout=100)  # errored run: no timing, no cache counts
        b["cot"] = "Run failed with error: llm down"
        res = {"a": results([a, b])}
        s = report.summarize(res, price_in=5.0, price_out=25.0)["total"]
        # defaults: read 0.1x = 0.5, write 1.25x = 6.25 per 1M
        want = (2000 * 5.0 + 200 * 25.0 + 2_000_000 * 0.5 + 1_000_000 * 6.25) / 1e6
        self.assertAlmostEqual(s["cost"], want)
        self.assertEqual(s["run_failures"], 1)
        s2 = report.summarize(res, 5.0, 25.0, price_cache_read=1.0, price_cache_write=2.0)["total"]
        self.assertAlmostEqual(s2["cost"], (2000 * 5.0 + 200 * 25.0 + 2e6 * 1.0 + 1e6 * 2.0) / 1e6)
        out = report.render(res, "m", 5.0, 25.0)
        self.assertIn("1 run failures carry no usage; their entire cost is missing, so $ is a "
                      "lower bound.", out)
        self.assertIn("2,000,000 / 1,000,000", out)


class FailureKindTests(unittest.TestCase):
    def test_kinds(self):
        self.assertEqual(report.failure_kind("Run failed with error: max iterations exceeded"),
                         "max iterations exceeded")
        self.assertEqual(report.failure_kind("Run failed with error: agentic pipeline asked a "
                                             "clarifying question: x"), "agent asked a clarifying question")


class TimingTests(unittest.TestCase):
    def setUp(self):
        r1 = rec(qid=1); r1["timing"] = timing()  # other = 500
        r2 = rec(qid=2); r2["timing"] = timing(wall=20000, llm=15000, fanout=True)
        r3 = rec(qid=3, choice="FAIL"); r3["timing"] = timing(wall=30000, llm=1000, tool=100,
                                                              query=100, startup=100)
        r4 = rec(qid=4)  # errored run: no timing, still graded
        self.recs = [r1, r2, r3, r4]

    def test_counts_and_wall_percentiles(self):
        s = report.timing_summary(self.recs)
        self.assertEqual(s["timed"], 3)
        self.assertEqual(s["untimed"], 1)
        self.assertAlmostEqual(s["wall_median_s"], 20.0)
        self.assertEqual(s["fanout_runs"], 1)

    def test_other_only_over_non_fanout(self):
        s = report.timing_summary(self.recs)
        self.assertEqual(s["other_runs"], 2)
        # other: r1 500 + r3 (30000-100-1000-100-100=28700), over wall 10000+30000
        self.assertAlmostEqual(s["share"]["other"], (500 + 28700) / 40000)
        self.assertAlmostEqual(s["share"]["llm"], (6000 + 15000 + 1000) / 60000)

    def test_llm_detail_and_coverage(self):
        s = report.timing_summary(self.recs)
        self.assertEqual(s["ttft_coverage"], (6, 12))  # ttft_rounds / llm_calls
        self.assertAlmostEqual(s["ttft_mean_ms"], 2400 / 6)
        self.assertAlmostEqual(s["calls_mean"], 4.0)

    def test_slowest_questions_dominant_component(self):
        s = report.timing_summary(self.recs)
        self.assertEqual(s["slowest"][0]["question_id"], 3)
        self.assertEqual(s["slowest"][0]["dominant"], "other")
        self.assertEqual(s["slowest"][1]["dominant"], "llm")

    def test_render_has_timing_section(self):
        out = report.render({"a": results(self.recs)}, "m", 1.0, 1.0)
        self.assertIn("## Timing", out)
        self.assertIn("6 / 12", out)
        self.assertIn("## Bottleneck", out)


class ReportTests(unittest.TestCase):
    def setUp(self):
        a = [rec(qid=i, choice="PASS" if i < 5 else "FAIL",
                 verdict="CORRECT" if i < 5 else "INCORRECT") for i in range(10)]
        # B: 30 graded; 1 JUDGE_ERROR (excluded from judged), 1 SKIP (excluded from graded).
        b = [rec(qid=100 + i) for i in range(29)]
        b.append(rec(qid=200, verdict="JUDGE_ERROR", reason="429"))
        b.append(rec(qid=300, choice="SKIP", verdict=None, tin=0, tout=0, reason="gold sql failed"))
        self.res = {"a": results(a), "b": results(b)}

    def test_report_totals_weight_by_graded(self):
        t = report.summarize(self.res)
        self.assertEqual(t["a"]["graded"], 10)
        self.assertEqual(t["b"]["graded"], 30)
        self.assertEqual(t["b"]["skipped"], 1)
        self.assertAlmostEqual(t["total"]["ex"], (5 + 30) / 40)
        self.assertAlmostEqual(t["a"]["ex"], 0.5)
        # answer accuracy weights by judged (JUDGE_ERROR and SKIP excluded)
        self.assertEqual(t["b"]["judged"], 29)
        self.assertEqual(t["total"]["judged"], 39)
        self.assertAlmostEqual(t["total"]["answer_acc"], (5 + 29) / 39)

    def test_cost_from_tokens(self):
        t = report.summarize(self.res, price_in=3.0, price_out=15.0)
        self.assertEqual(t["total"]["input_tokens"], 40 * 100)
        self.assertAlmostEqual(t["total"]["cost"], (4000 * 3.0 + 400 * 15.0) / 1e6)

    def test_render_lists(self):
        out = report.render(self.res, model="m", price_in=3.0, price_out=15.0)
        self.assertIn("judge-x", out)
        self.assertIn("Skipped (gold error)", out)
        self.assertIn("300", out)
        self.assertIn("Judge errors", out)
        self.assertIn("Failures", out)
        self.assertIn("87.2%", out)  # 34/39 answer accuracy

    def test_correct_but_ex_fail_counted(self):
        r = {"a": results([rec(choice="FAIL", verdict="CORRECT"), rec()])}
        self.assertEqual(report.summarize(r)["total"]["correct_ex_fail"], 1)

    def test_ex_reads_score_not_choice(self):
        # New results files: `choice` follows the judge, `score` is the EX bit.
        r = rec(choice="PASS", verdict="CORRECT")
        r["score"] = 0.0
        t = report.summarize({"a": results([r])})["total"]
        self.assertEqual((t["ex"], t["correct_ex_fail"]), (0.0, 1))


class MeasuredRunTests(unittest.TestCase):
    def test_latency_and_rounds_skip_run_failures(self):
        ok = [rec(qid=1, ms=10_000, rounds=4), rec(qid=2, ms=30_000, rounds=8)]
        failed = rec(qid=3, choice="FAIL", verdict="INCORRECT", ms=0.0, rounds=0, tin=0, tout=0)
        failed["cot"] = "Run failed with error: max iterations exceeded"
        t = report.summarize({"a": results(ok + [failed])})["total"]
        self.assertEqual((t["graded"], t["measured"], t["run_failures"]), (3, 2, 1))
        self.assertAlmostEqual(t["lat_mean"], 20.0)
        self.assertAlmostEqual(t["lat_median"], 20.0)
        self.assertAlmostEqual(t["rounds"], 6.0)
        self.assertAlmostEqual(t["answer_acc"], 2 / 3)  # still counts against accuracy
        out = report.render({"a": results(ok + [failed])}, "m", 1.0, 1.0)
        self.assertIn("| 2 | 20.0 / 20.0 | 6.0 |", out)


FIXTURES = Path(__file__).resolve().parent / "fixtures"


def with_runtime(records, runtime):
    return [dict(r, runtime=runtime) for r in records]


class RuntimeTests(unittest.TestCase):
    def write(self, tmp, name, records):
        (Path(tmp) / name).write_text(json.dumps(results(records)))

    def test_load_results_rejects_a_runtime_mismatch(self):
        with tempfile.TemporaryDirectory() as tmp:
            self.write(tmp, "financial.react.test.results.json",
                       with_runtime([rec(qid=1)], "fsm"))
            with self.assertRaises(ValueError):
                report.load_results("react", Path(tmp), dbs=["financial"])
            self.write(tmp, "financial.test.results.json", with_runtime([rec(qid=1)], "react"))
            with self.assertRaises(ValueError):
                report.load_results("fsm", Path(tmp), dbs=["financial"])

    def test_react_file_loads_and_failed_runs_are_exempt(self):
        failed = rec(qid=2, choice="FAIL", verdict="INCORRECT", ms=0.0, rounds=0)
        failed["cot"] = "Run failed with error: x"
        failed["answer_text"] = None  # a failed run never recorded a runtime
        with tempfile.TemporaryDirectory() as tmp:
            self.write(tmp, "financial.react.test.results.json",
                       with_runtime([rec(qid=1)], "react") + [failed])
            out = report.load_results("react", Path(tmp), dbs=["financial"])
        self.assertEqual(len(report._execution(out["financial"])["records"]), 2)

    def test_archived_records_without_runtime_load_as_fsm(self):
        with mock.patch.object(report.cases, "load_rows", return_value=[]):
            res = report.load_results("fsm", FIXTURES / "fsm_archive", dbs=["financial"])
        recs = report._execution(res["financial"])["records"]
        self.assertTrue(recs and all("runtime" not in r for r in recs))
        s = report.summarize(res)["total"]
        self.assertEqual((s["correct"], s["judged"]), (2, 3))
        # The same records, read as react, are a mismatch.
        with self.assertRaises(ValueError):
            report.check_runtime(recs, "react", "archive")

    def test_render_title_follows_runtime(self):
        res = {"a": results([rec()])}
        self.assertIn("# BIRD Mini-Dev FSM baseline", report.render(res, "m", 1.0, 1.0))
        react = {"a": results(with_model([rec()], "m"))}  # react records carry their model
        self.assertIn("# BIRD Mini-Dev react runtime",
                      report.render(react, "m", 1.0, 1.0, runtime="react"))


class CompareTests(unittest.TestCase):
    def test_compare_lists_differing_verdicts(self):
        fsm = {"financial": results([rec(qid=1), rec(qid=2), rec(qid=3, rounds=4)])}
        react = {"financial": results(with_model([
            rec(qid=1), rec(qid=2, choice="FAIL", verdict="INCORRECT"), rec(qid=3, rounds=8)],
            "m"))}
        self.assertEqual(compare.differing(fsm, react),
                         [("financial", 2, ["CORRECT"], ["INCORRECT"])])
        out = compare.render(fsm, react, "m", 1.0, 1.0)
        self.assertIn("(1)", out)
        self.assertIn("- financial 2: fsm CORRECT, react INCORRECT", out)
        self.assertNotIn("financial 1:", out)
        for label in ("answer accuracy", "EX", "Soft-F1", "median latency s", "median turns",
                      "tokens in / out", "cost", "### pooled", "### financial"):
            self.assertIn(label, out)

    def test_median_turns_per_arm(self):
        fsm = {"a": results([rec(qid=i, rounds=r) for i, r in enumerate([2, 4, 10])])}
        react = {"a": results(with_model([rec(qid=i, rounds=r)
                                          for i, r in enumerate([6, 8, 9])], "m"))}
        out = compare.render(fsm, react, "m", 1.0, 1.0)
        self.assertIn("| median turns | 4.0 | 8.0 |", out)


def with_model(records, model):
    return [dict(r, model=model) for r in records]


class SettingsTests(unittest.TestCase):
    def test_compare_refuses_when_the_arms_ran_different_models(self):
        fsm = {"a": results(with_model([rec(qid=1)], "model-x"))}
        react = {"a": results(with_model([rec(qid=1)], "model-y"))}
        with self.assertRaisesRegex(ValueError, "different models"):
            compare.check_same_model(fsm, react)
        with self.assertRaises(ValueError):  # render refuses too, through the declared model
            compare.render(fsm, react, "model-x", 1.0, 1.0)

    def test_compare_prints_models_thinking_and_judge(self):
        fsm = {"a": results(with_model([rec(qid=1)], "model-x"))}
        react = {"a": results(with_model([rec(qid=1)], "model-x"))}
        out = compare.render(fsm, react, "model-x", 1.0, 1.0)
        self.assertIn("Recorded model: `model-x`", out)
        self.assertIn("Thinking:", out)
        self.assertIn("Judge model: `judge-x`", out)
        self.assertIn("excluded from the median", out)

    def test_unrecorded_baseline_is_printed_when_react_matches_the_declared_model(self):
        fsm = {"a": results([rec(qid=1)])}
        react = {"a": results(with_model([rec(qid=1)], "model-y"))}
        self.assertIn("unrecorded", compare.render(fsm, react, "model-y", 1.0, 1.0))

    def test_compare_refuses_a_recorded_model_that_is_not_the_declared_one(self):
        archive = {"a": results([rec(qid=1)])}  # unrecorded
        react = {"a": results(with_model([rec(qid=1)], "model-x"))}
        with self.assertRaisesRegex(ValueError, "react: .*model-x.*--model is 'model-y'"):
            compare.render(archive, react, "model-y", 1.0, 1.0)
        with self.assertRaisesRegex(ValueError, "fsm: "):
            compare.render(react, archive, "model-y", 1.0, 1.0)

    def test_a_react_record_without_a_model_is_refused(self):
        unrecorded = {"a": results([rec(qid=1)])}
        fsm = {"a": results(with_model([rec(qid=1)], "model-x"))}
        with self.assertRaisesRegex(ValueError, "react: .*unrecorded"):
            compare.render(fsm, unrecorded, "model-x", 1.0, 1.0)
        with self.assertRaisesRegex(ValueError, "react: .*unrecorded"):
            report.render(unrecorded, "model-x", 1.0, 1.0, runtime="react")
        report.render(unrecorded, "model-x", 1.0, 1.0, runtime="fsm")  # the archived baseline

    def test_report_refuses_a_recorded_model_that_is_not_the_declared_one(self):
        res = {"a": results(with_model([rec()], "model-x"))}
        for runtime in ("fsm", "react"):
            with self.assertRaisesRegex(ValueError, "--model is 'model-y'"):
                report.render(res, "model-y", 1.0, 1.0, runtime=runtime)
        report.render(res, "model-x", 1.0, 1.0)

    def test_thinking_line_says_where_it_comes_from(self):
        out = compare.render({"a": results(with_model([rec()], "m"))},
                             {"a": results(with_model([rec()], "m"))}, "m", 1.0, 1.0)
        self.assertIn("solving state:", out)
        self.assertIn("from the agent files in this checkout", out)

    def test_report_prints_the_recorded_model(self):
        res = {"a": results(with_model([rec()], "model-x"))}
        self.assertIn("Recorded model: `model-x`", report.render(res, "model-x", 1.0, 1.0))

    def test_an_all_failed_react_file_raises(self):
        failed = rec(qid=1, choice="FAIL", verdict="INCORRECT", ms=0.0, rounds=0)
        failed["cot"] = "Run failed with error: x"
        failed["answer_text"] = None
        with self.assertRaisesRegex(ValueError, "labeled react"):
            report.check_runtime([failed], "react", "f")
        report.check_runtime([failed], "fsm", "f")

    def test_react_timing_without_ttft_is_n_a(self):
        r = dict(rec(qid=1), model="m")
        r["timing"] = timing(ttft_rounds=0, ttft=0, stream=0)
        out = report.render({"a": results([r])}, "m", 1.0, 1.0, runtime="react")
        self.assertIn("TTFT mean n/a (react)", out)


class CliTests(unittest.TestCase):
    def test_a_refusal_prints_an_error_and_exits_2(self):
        import contextlib
        import io

        import bird
        err = io.StringIO()
        argv = ["compare", "--fsm-dir", "x", "--react-dir", "y", "--model", "m",
                "--price-in", "1", "--price-out", "1"]
        with mock.patch("birdlib.compare.run", side_effect=ValueError("arms differ")), \
                contextlib.redirect_stderr(err):
            self.assertEqual(bird.main(argv), 2)
        self.assertEqual(err.getvalue(), "error: arms differ\n")


class QuerySourceTests(unittest.TestCase):
    def _rec(self, qid, verdict, source, execs=None):
        r = rec(qid=qid, verdict=verdict)
        r["timing"] = timing()
        if source:
            r["timing"]["answer_source"] = source
        r["timing"]["queries_by_source"] = execs or {}
        return r

    def test_accuracy_by_answer_source(self):
        recs = [self._rec(1, "CORRECT", "semantic", {"semantic": 2, "llm": 1}),
                self._rec(2, "INCORRECT", "semantic", {"semantic": 1}),
                self._rec(3, "CORRECT", "llm", {"llm": 3}),
                self._rec(4, "JUDGE_ERROR", "llm"),
                self._rec(5, "INCORRECT", None),
                rec(qid=6, choice="SKIP", verdict=None)]
        s = report.source_summary(recs)
        self.assertEqual(s["answers"]["semantic"], {"answers": 2, "judged": 2, "correct": 1})
        self.assertEqual(s["answers"]["llm"], {"answers": 2, "judged": 1, "correct": 1})
        self.assertEqual(s["answers"][report.NOT_RECORDED]["answers"], 1)
        self.assertEqual(s["executions"], {"semantic": 3, "llm": 4})
        out = "\n".join(report.render_sources(recs))
        self.assertIn("| semantic | 2 | 1/2 | 50.0% |", out)
        self.assertIn("| llm | 2 | 1/1 | 100.0% |", out)
        self.assertLess(out.index("| llm |"), out.index("| not recorded |"))
        self.assertIn("Executions by source (every query run, failures included): llm 4, "
                      "semantic 3.", out)

    def test_old_results_say_not_recorded(self):
        old = rec(qid=1)
        old["timing"] = timing()
        out = report.render_sources([old, rec(qid=2)])
        self.assertIn("Not recorded: these results predate", "\n".join(out))


if __name__ == "__main__":
    unittest.main()
