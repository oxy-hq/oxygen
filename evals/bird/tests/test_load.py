import unittest
from unittest import mock

from birdlib import config, load


class EnsureRoleAndGrantTests(unittest.TestCase):
    def _run_load(self, existing: set[str]):
        calls = []

        def fake_psql(db, sql, **kw):
            calls.append((db, sql))
            return mock.Mock(returncode=0, stdout="", stderr="")

        with mock.patch.object(load, "_psql", side_effect=fake_psql), \
                mock.patch.object(load, "ensure_container"), \
                mock.patch.object(load, "_db_exists", side_effect=lambda pgdb: pgdb in existing):
            load.load(fresh=False)
        return calls

    def test_role_timeout_applied_outside_create_guard(self):
        calls = self._run_load({config.pg_database(db) for db in config.DBS})
        role_sql = calls[0][1]
        stmt = f"ALTER ROLE {config.RO_ROLE} SET statement_timeout = '60s';"
        self.assertIn(stmt, role_sql)
        self.assertGreater(role_sql.index(stmt), role_sql.index("END $$;"))

    def test_existing_dbs_are_re_granted_without_fresh(self):
        calls = self._run_load({config.pg_database(db) for db in config.DBS})
        granted = [db for db, sql in calls if "GRANT SELECT ON ALL TABLES" in sql]
        self.assertEqual(granted, [config.pg_database(db) for db in config.DBS])
        self.assertFalse(any("DROP DATABASE" in sql or "CREATE DATABASE" in sql for _, sql in calls))


if __name__ == "__main__":
    unittest.main()
