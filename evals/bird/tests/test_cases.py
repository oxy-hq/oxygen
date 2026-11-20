import unittest
from pathlib import Path

from birdlib import cases, config


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

    def test_gen_cases_react_targets_the_react_agent(self):
        fsm = cases.render_test_file("financial", ROWS, set())
        react = cases.render_test_file("financial", ROWS, set(), runtime="react")
        self.assertIn("target: financial.react.agentic.yml", react)
        self.assertIn('name: "bird-react-financial"', react)
        names = lambda t: [l for l in t.splitlines() if l.strip().startswith("- name:")]
        self.assertEqual(names(react), names(fsm))
        # Everything but the suite name and target is identical.
        body = lambda t: t.split("settings:", 1)[1]
        self.assertEqual(body(react), body(fsm))

    def test_react_agents_differ_only_by_runtime(self):
        root = Path(__file__).resolve().parent.parent
        for db in config.DBS:
            fsm = (root / f"{db}.agentic.yml").read_text().splitlines()
            react = (root / f"{db}.react.agentic.yml").read_text().splitlines()
            self.assertEqual(react.count("runtime: react"), 1, db)
            self.assertEqual([l for l in react if l != "runtime: react"], fsm, db)
            self.assertFalse(any(l.startswith("runtime:") for l in fsm), db)


if __name__ == "__main__":
    unittest.main()
