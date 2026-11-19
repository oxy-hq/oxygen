import unittest

from birdlib import report


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


if __name__ == "__main__":
    unittest.main()
