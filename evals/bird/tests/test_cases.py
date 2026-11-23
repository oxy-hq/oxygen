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

    def test_halves_split_by_id_and_cover_every_case_once(self):
        rows = [row(i, "financial") for i in (5, 1, 4, 2, 3)]
        a = cases.render_test_file("financial", rows, set(), runtime="semantic", half="a")
        b = cases.render_test_file("financial", rows, set(), runtime="semantic", half="b")
        self.assertIn('name: "bird-semantic-financial-half-a"', a)
        for qid in (1, 2, 3):
            self.assertIn(f'"bird-{qid}"', a)
            self.assertNotIn(f'"bird-{qid}"', b)
        for qid in (4, 5):
            self.assertIn(f'"bird-{qid}"', b)
            self.assertNotIn(f'"bird-{qid}"', a)

    def test_half_file_names(self):
        self.assertEqual(cases.test_file("financial", "semantic", "a"),
                         "financial.semantic.half-a.test.yml")
        self.assertEqual(cases.test_file("financial"), "financial.test.yml")

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

    def test_semantic_agents_differ_from_react_only_by_semantic_only(self):
        root = Path(__file__).resolve().parent.parent
        for db in config.DBS:
            react = (root / f"{db}.react.agentic.yml").read_text().splitlines()
            sem = (root / f"{db}.semantic.agentic.yml").read_text().splitlines()
            self.assertEqual(sem.count("semantic_only: true"), 1, db)
            self.assertEqual([l for l in sem if l != "semantic_only: true"], react, db)

    def test_gen_cases_semantic_targets_the_semantic_agent(self):
        sem = cases.render_test_file("financial", ROWS, set(), runtime="semantic")
        self.assertIn("target: financial.semantic.agentic.yml", sem)
        self.assertIn('name: "bird-semantic-financial"', sem)
        self.assertEqual(cases.test_file("financial", "semantic"), "financial.semantic.test.yml")


if __name__ == "__main__":
    unittest.main()
