import unittest

from birdlib import evidence

ROWS = [
    {"question_id": 1, "db_id": "financial", "question": "QUESTION_SENTINEL one?",
     "evidence": "weekly issuance refers to 'POPLATEK TYDNE'", "SQL": "SELECT 1"},
    {"question_id": 2, "db_id": "financial", "question": "QUESTION_SENTINEL two?",
     "evidence": "weekly issuance refers to 'POPLATEK TYDNE'", "SQL": "SELECT 2"},
    {"question_id": 3, "db_id": "financial", "question": "QUESTION_SENTINEL three?",
     "evidence": "", "SQL": "SELECT 3"},
    {"question_id": 4, "db_id": "formula_1", "question": "OTHER_DB?",
     "evidence": "OTHER_DB_EVIDENCE", "SQL": "SELECT 4"},
]
CSVS = [("account", "original_column_name,column_description\nfrequency,frequency of statement\n")]


class EvidenceTests(unittest.TestCase):
    def test_evidence_has_no_questions_or_sql(self):
        out = evidence.render("financial", ROWS, CSVS)
        self.assertIn("weekly issuance refers to 'POPLATEK TYDNE'", out)
        self.assertIn("frequency of statement", out)
        self.assertEqual(out.count("POPLATEK TYDNE"), 1)
        self.assertNotIn("QUESTION_SENTINEL", out)
        self.assertNotIn("OTHER_DB", out)
        self.assertNotIn("SELECT", out.upper())

    def test_decode_falls_back_for_non_utf8(self):
        self.assertEqual(evidence.decode("caf\xe9".encode("cp1252")), "caf\xe9")


if __name__ == "__main__":
    unittest.main()
