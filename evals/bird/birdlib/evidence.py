"""Print a db's evidence strings and description CSVs. Never questions or SQL."""
import csv
import io
import zipfile

from . import config

DESC_DIR = "minidev/MINIDEV/dev_databases/{db}/database_description/"


def evidence_strings(rows: list[dict], db: str) -> list[str]:
    """Sorted, de-duplicated, non-empty evidence strings for one db."""
    seen = {(r.get("evidence") or "").strip() for r in rows if r["db_id"] == db}
    return sorted(s for s in seen if s)


def decode(raw: bytes) -> str:
    for enc in ("utf-8-sig", "cp1252"):
        try:
            return raw.decode(enc)
        except UnicodeDecodeError:
            continue
    return raw.decode("latin-1")


def description_csvs(db: str) -> list[tuple[str, str]]:
    """(table name, csv text) for each description CSV, from the cached zip."""
    prefix = DESC_DIR.format(db=db)
    out = []
    with zipfile.ZipFile(config.CACHE / config.ZIP_NAME) as z:
        for name in sorted(z.namelist()):
            if name.startswith(prefix) and name.endswith(".csv"):
                out.append((name[len(prefix):-4], decode(z.read(name))))
    return out


def render(db: str, rows: list[dict], csvs: list[tuple[str, str]]) -> str:
    out = [f"# Evidence ({db})", ""]
    out += [f"- {s}" for s in evidence_strings(rows, db)]
    out += ["", f"# Column descriptions ({db})"]
    for table, text in csvs:
        out += ["", f"## {table}"]
        for rec in csv.reader(io.StringIO(text)):
            out.append(" | ".join(c.strip().replace("\n", " ") for c in rec))
    return "\n".join(out) + "\n"


def print_evidence(db: str) -> None:
    from .cases import load_rows
    print(render(db, load_rows(), description_csvs(db)), end="")
