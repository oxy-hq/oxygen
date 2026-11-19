import unittest

from birdlib import cases


def row(qid, db, difficulty="simple", evidence="EVIDENCE_SENTINEL"):
    return {"question_id": qid, "db_id": db, "question": f"q{qid}?", "evidence": evidence,
            "SQL": f"SELECT {qid}", "difficulty": difficulty}


ROWS = [row(1, "financial"), row(2, "financial", "challenging"), row(3, "formula_1"),
        row(4, "financial", "moderate")]


class CasesTests(unittest.TestCase):
    def test_render_excludes_disputed_and_other_dbs(self):
        out = cases.render_test_file("financial", ROWS, {2})
        self.assertIn('"bird-1"', out)
        self.assertIn('"bird-4"', out)
        self.assertNotIn("bird-2", out)
        self.assertNotIn("bird-3", out)

    def test_render_never_contains_evidence(self):
        out = cases.render_test_file("financial", ROWS, set())
        self.assertNotIn("EVIDENCE_SENTINEL", out)
        self.assertNotIn("evidence", out)

    def test_render_header(self):
        out = cases.render_test_file("financial", ROWS, set())
        for s in ["target: financial.agentic.yml", "grader: execution",
                  "database: bird_financial", "runs: 1",
                  'name: "bird-1"', 'prompt: "q1?"', 'expected_sql: "SELECT 1"',
                  'tags: ["simple", "financial"]']:
            self.assertIn(s, out)

    def test_disputed_entries_accept_ids_and_objects(self):
        self.assertEqual(cases.disputed_ids([1, {"question_id": 2, "reason": "x"}]), {1, 2})

    def test_runs_override(self):
        self.assertIn("runs: 3", cases.render_test_file("financial", ROWS, set(), runs=3))


if __name__ == "__main__":
    unittest.main()
