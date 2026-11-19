"""Load the split BIRD dump into a Docker Postgres and smoke-check it as the read-only role."""
import json
import subprocess
import time
import zipfile
from collections.abc import Iterator

from . import config, split


def _docker(*args: str, input: str | None = None, check: bool = True) -> subprocess.CompletedProcess:
    return subprocess.run(
        ["docker", *args], input=input, capture_output=True, text=True, check=check
    )


def _psql(db: str, sql: str, *, user: str = config.PG_SUPERUSER, password: str | None = None,
          check: bool = True) -> subprocess.CompletedProcess:
    """Run `sql` through psql inside the container, over TCP so `password` is exercised."""
    env = ["-e", f"PGPASSWORD={password or config.PG_SUPERPASSWORD}"]
    return _docker(
        "exec", "-i", *env, config.PG_CONTAINER,
        "psql", "-X", "-q", "-A", "-t", "-v", "ON_ERROR_STOP=1", "-h", "127.0.0.1", "-U", user, "-d", db, "-f", "-",
        input=sql, check=check,
    )


def ensure_container() -> None:
    state = _docker("inspect", "-f", "{{.State.Running}}", config.PG_CONTAINER, check=False)
    if state.returncode != 0:
        print(f"starting {config.PG_CONTAINER} on port {config.PG_PORT}")
        _docker(
            "run", "-d", "--name", config.PG_CONTAINER,
            "-e", f"POSTGRES_PASSWORD={config.PG_SUPERPASSWORD}",
            "-p", f"127.0.0.1:{config.PG_PORT}:5432", config.PG_IMAGE,
        )
    elif state.stdout.strip() != "true":
        _docker("start", config.PG_CONTAINER)
    wait_ready()


def wait_ready(timeout: float = 90) -> None:
    # -h 127.0.0.1: the image's init-time temp server listens on the socket only.
    deadline = time.time() + timeout
    while time.time() < deadline:
        r = _docker("exec", config.PG_CONTAINER, "pg_isready", "-h", "127.0.0.1",
                    "-U", config.PG_SUPERUSER, check=False)
        if r.returncode == 0:
            return
        time.sleep(1)
    raise SystemExit(f"{config.PG_CONTAINER} not ready after {timeout}s")


def _db_exists(db: str) -> bool:
    r = _psql("postgres", f"SELECT 1 FROM pg_database WHERE datname = '{db}';")
    return r.stdout.strip() == "1"


def _ensure_role() -> None:
    """Create the read-only role if missing, and (re)apply its settings every time. The
    statement_timeout lets the server cancel a query the grader abandoned at its own 60 s
    timeout; it bounds the agent's queries too. It applies to new sessions only."""
    _psql("postgres", f"""
DO $$ BEGIN
  IF NOT EXISTS (SELECT FROM pg_roles WHERE rolname = '{config.RO_ROLE}') THEN
    CREATE ROLE {config.RO_ROLE} LOGIN PASSWORD '{config.RO_PASSWORD}';
  END IF;
END $$;
ALTER ROLE {config.RO_ROLE} SET statement_timeout = '{config.RO_STATEMENT_TIMEOUT}';
""")


def _grant_read_only(pgdb: str) -> None:
    _psql(pgdb, f"""
GRANT CONNECT, TEMP ON DATABASE {pgdb} TO {config.RO_ROLE};
GRANT USAGE ON SCHEMA public TO {config.RO_ROLE};
GRANT SELECT ON ALL TABLES IN SCHEMA public TO {config.RO_ROLE};
""")


def _split_from_zip(dbs: set[str]) -> dict[str, list[str]]:
    zip_path = config.CACHE / config.ZIP_NAME
    if not zip_path.exists():
        raise SystemExit(f"{zip_path} missing; run `bird.py fetch` first")
    with zipfile.ZipFile(zip_path) as z:
        tables = json.loads(z.read(config.ZIP_TABLES_MEMBER))
        lines: Iterator[str] = (
            raw.decode("utf-8") for raw in z.open(config.ZIP_DUMP_MEMBER)
        )
        return split.split_dump(lines, split.table_map(tables), dbs)


def _load_one(db: str, sql_lines: list[str]) -> None:
    pgdb = config.pg_database(db)
    _psql("postgres", f"CREATE DATABASE {pgdb};")
    try:
        r = _psql(pgdb, "".join(sql_lines), check=False)
        if r.returncode != 0:
            raise SystemExit(f"loading {pgdb} failed:\n{r.stderr[-2000:]}")
        _grant_read_only(pgdb)
    except BaseException:
        _psql("postgres", f"DROP DATABASE IF EXISTS {pgdb} WITH (FORCE);", check=False)
        raise


def load(fresh: bool = False) -> None:
    ensure_container()
    _ensure_role()
    todo = set()
    for db in config.DBS:
        pgdb = config.pg_database(db)
        if _db_exists(pgdb):
            if not fresh:
                print(f"{pgdb}: exists, skipping load (use --fresh to re-create)")
                _grant_read_only(pgdb)  # idempotent; keeps an old load's grants current
                continue
            _psql("postgres", f"DROP DATABASE {pgdb} WITH (FORCE);")
        todo.add(db)
    if not todo:
        return
    print(f"splitting dump for {sorted(todo)}")
    parts = _split_from_zip(todo)
    for db in config.DBS:
        if db not in todo:
            continue
        if db not in parts:
            raise SystemExit(f"no statements for {db} in the dump")
        print(f"{config.pg_database(db)}: loading {len(parts[db])} lines")
        _load_one(db, parts.pop(db))
    print("load done")


def _tables(pgdb: str) -> list[str]:
    r = _psql(pgdb, "SELECT tablename FROM pg_tables WHERE schemaname = 'public' ORDER BY 1;",
              user=config.RO_ROLE, password=config.RO_PASSWORD)
    return r.stdout.split()


def _ro(pgdb: str, sql: str) -> subprocess.CompletedProcess:
    return _psql(pgdb, sql, user=config.RO_ROLE, password=config.RO_PASSWORD, check=False)


def _gold_rows() -> list[dict]:
    path = config.CACHE / config.JSONL_NAME
    with open(path) as f:
        return [json.loads(l) for l in f if l.strip()]


def smoke() -> int:
    ok = True
    gold = _gold_rows()
    for db in config.DBS:
        pgdb = config.pg_database(db)
        tables = _tables(pgdb)
        if not tables:
            print(f"{pgdb}: FAIL no tables")
            ok = False
            continue
        counts = {}
        for t in tables:
            r = _ro(pgdb, f'SELECT count(*) FROM public."{t}";')
            counts[t] = int(r.stdout.strip()) if r.returncode == 0 else -1
        print(f"{pgdb}: row counts {counts}")
        if any(c <= 0 for c in counts.values()):
            print(f"{pgdb}: FAIL empty or unreadable table(s)")
            ok = False
        d = _ro(pgdb, f'DELETE FROM public."{tables[0]}";')
        denied = d.returncode != 0 and "permission denied" in d.stderr
        print(f"{pgdb}: DELETE FROM {tables[0]} -> "
              f"{'permission denied (ok)' if denied else 'NOT DENIED: ' + d.stderr.strip()}")
        ok &= denied
        t = _ro(pgdb, "CREATE TEMP TABLE bird_smoke_t AS SELECT 1;")
        print(f"{pgdb}: CREATE TEMP TABLE -> {'ok' if t.returncode == 0 else 'FAILED: ' + t.stderr.strip()}")
        ok &= t.returncode == 0
        i = _ro(pgdb, f'INSERT INTO public."{tables[0]}" SELECT * FROM public."{tables[0]}" LIMIT 1;')
        ins_denied = i.returncode != 0 and "permission denied" in i.stderr
        print(f"{pgdb}: INSERT -> {'permission denied (ok)' if ins_denied else 'NOT DENIED'}")
        ok &= ins_denied
        failed = []
        rows = [g for g in gold if g["db_id"] == db]
        for g in rows:
            r = _ro(pgdb, f"SET statement_timeout = '120s';\n{g['SQL'].rstrip().rstrip(';')};\n")
            if r.returncode != 0:
                failed.append(g["question_id"])
        extra = sorted(set(failed) - config.CROSS_DIALECT_IDS)
        print(f"{pgdb}: gold SQL {len(rows) - len(failed)}/{len(rows)} ok; "
              f"failed question_ids {sorted(failed)}")
        if extra:
            print(f"{pgdb}: FAIL unexpected gold failures {extra}")
            ok = False
    print("smoke " + ("ok" if ok else "FAILED"))
    return 0 if ok else 1
