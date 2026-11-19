import json
import unittest
from pathlib import Path

from birdlib import split

FIX = Path(__file__).parent / "fixtures"


def _dump_lines():
    return (FIX / "mini_dump.sql").read_text().splitlines(keepends=True)


def _table_map():
    return split.table_map(json.loads((FIX / "dev_tables.json").read_text()))


WANTED = {"financial", "debit_card_specializing"}


class SplitTests(unittest.TestCase):
    def test_split_routes_tables(self):
        out = split.split_dump(_dump_lines(), _table_map(), WANTED)
        self.assertEqual(set(out), WANTED)
        fin = "".join(out["financial"])
        self.assertIn("SET statement_timeout = 0;", fin)
        self.assertIn("search_path", fin)
        self.assertIn("CREATE TABLE public.account (", fin)
        self.assertIn("COPY public.account (account_id, frequency) FROM stdin;", fin)
        self.assertIn("account_pkey", fin)
        self.assertNotIn("OWNER TO", fin)
        self.assertNotIn("customers_pkey", fin)
        self.assertNotIn("idx_customers_segment", fin)
        self.assertNotIn("atom", fin)
        self.assertNotIn("SME", fin)
        deb = "".join(out["debit_card_specializing"])
        self.assertIn("SET statement_timeout = 0;", deb)
        self.assertIn("idx_customers_segment", deb)
        self.assertIn("customers_pkey", deb)
        self.assertNotIn("account", deb)
        self.assertNotIn("atom", deb)
        self.assertNotIn("toxicology", out)

    def test_split_handles_copy_rows_containing_keywords(self):
        out = split.split_dump(_dump_lines(), _table_map(), WANTED)
        fin = out["financial"]
        keyword_row = "2\tCREATE TABLE public.customers (id int);\n"
        self.assertIn(keyword_row, fin)
        i = fin.index(keyword_row)
        copy_start = max(j for j, l in enumerate(fin[:i]) if l.startswith("COPY public.account"))
        end = fin.index("\\.\n", i)
        self.assertGreater(end, i)
        self.assertGreater(i, copy_start)
        # the row did not open a second CREATE TABLE for customers in financial's output
        self.assertEqual(sum(1 for l in fin if l.startswith("CREATE TABLE public.customers")), 0)
        deb = "".join(out["debit_card_specializing"])
        self.assertNotIn("id int", deb)

    def test_split_routes_sequences_to_their_table(self):
        out = split.split_dump(_dump_lines(), _table_map(), WANTED)
        deb = "".join(out["debit_card_specializing"])
        self.assertIn("CREATE SEQUENCE public.customers_customerid_seq", deb)
        self.assertIn("OWNED BY public.customers.customerid", deb)
        self.assertIn("nextval('public.customers_customerid_seq'", deb)
        self.assertIn("pg_catalog.setval('public.customers_customerid_seq'", deb)
        self.assertNotIn("customerid_seq", "".join(out["financial"]))

    def test_table_map_lowercases(self):
        m = split.table_map(
            [{"db_id": "thrombosis_prediction", "table_names_original": ["Examination"]}]
        )
        self.assertEqual(m, {"examination": "thrombosis_prediction"})


if __name__ == "__main__":
    unittest.main()
