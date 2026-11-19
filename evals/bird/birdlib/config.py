"""Shared constants for the BIRD data loader. Names here are consumed by later tasks."""
from pathlib import Path

DBS = ["financial", "debit_card_specializing", "california_schools", "formula_1"]

PG_PORT = 15433
PG_CONTAINER = "oxy-bird-pg"
PG_IMAGE = "postgres:16"
PG_SUPERUSER = "postgres"
PG_SUPERPASSWORD = "postgres"
RO_ROLE = "bird_ro"
RO_PASSWORD = "bird_ro"
# Matches the grader's 60 s query timeout, so the server cancels a statement the grader dropped.
RO_STATEMENT_TIMEOUT = "60s"

# Downloaded data lives here and is never committed (BIRD is CC BY-SA 4.0).
CACHE = Path.home() / ".cache" / "oxy-evals" / "bird"

ZIP_URL = "https://bird-bench.oss-cn-beijing.aliyuncs.com/minidev.zip"
ZIP_CONTENT_LENGTH = 800943648
ZIP_SHA256 = "cc48ba16838204e4e214512030cb572eeb5f7bcdd999bae4b9b6ff12ec13b92f"
ZIP_NAME = "minidev.zip"

JSONL_URL = (
    "https://huggingface.co/datasets/birdsql/bird_mini_dev/resolve/main/"
    "data/mini_dev_postgresql.jsonl"
)
JSONL_SHA256 = "ff8b561724d8e643bc906434e9e612cc43a782a385764f1c288ffe82c73cbc2c"
JSONL_NAME = "mini_dev_postgresql.jsonl"

ZIP_DUMP_MEMBER = "minidev/MINIDEV_postgresql/BIRD_dev.sql"
ZIP_TABLES_MEMBER = "minidev/MINIDEV/dev_tables.json"

# Cross-dialect exception ids the HF manifest lists; smoke tolerates these failing.
CROSS_DIALECT_IDS = {132, 172, 175, 177, 184, 185, 216}


def pg_database(db: str) -> str:
    return f"bird_{db}"
