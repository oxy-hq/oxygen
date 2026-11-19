"""Split BIRD's single pg_dump (all DBs' tables in `public`) into one SQL stream per DB."""
import re
from collections.abc import Iterable

_TABLE_REF = re.compile(r'\bpublic\.(?:"([^"]+)"|([A-Za-z_][A-Za-z0-9_$]*))')
# Ownership/ACL statements name a role that does not exist in the target server.
_DROP = re.compile(r"\b(OWNER\s+TO|GRANT|REVOKE)\b", re.IGNORECASE)


def table_map(dev_tables_json: list[dict]) -> dict[str, str]:
    """Lowercased original table name -> db_id (the dump's names are lowercase)."""
    return {
        t.lower(): entry["db_id"]
        for entry in dev_tables_json
        for t in entry["table_names_original"]
    }


def _resolve(name: str, table_to_db: dict[str, str]) -> str | None:
    """A table name, or a sequence `<table>_<col>_seq` resolved to its longest table prefix."""
    if name in table_to_db:
        return name
    owners = [t for t in table_to_db if name.startswith(t + "_")]
    return max(owners, key=len) if owners else None


def _statement_table(text: str, table_to_db: dict[str, str]) -> str | None:
    for m in _TABLE_REF.finditer(text):
        owner = _resolve((m.group(1) or m.group(2)).lower(), table_to_db)
        if owner:
            return owner
    return None


def split_dump(
    dump_lines: Iterable[str], table_to_db: dict[str, str], wanted: set[str]
) -> dict[str, list[str]]:
    """Route each statement to the DB owning its table.

    A line state machine: `preamble | statement | copy`. A statement ends at a line
    ending `;`; a COPY data block ends at `\\.` and its rows are never parsed as SQL.
    Statements naming no known table before the first table statement are the preamble
    (given to every wanted DB); unrouted statements after it are dropped.
    """
    out: dict[str, list[str]] = {}
    preamble: list[str] = []
    comments: list[str] = []
    stmt: list[str] = []
    seen_table = False
    copy_db: str | None = None  # db receiving the current COPY block, if wanted
    in_copy = False

    def emit(db: str, lines: list[str]) -> None:
        out.setdefault(db, []).extend(lines)

    for line in dump_lines:
        if in_copy:
            if copy_db is not None:
                emit(copy_db, [line])
            if line.rstrip("\r\n") == "\\.":
                in_copy, copy_db = False, None
            continue
        if not stmt and (not line.strip() or line.startswith("--")):
            comments.append(line)
            continue
        stmt.append(line)
        if not line.rstrip().endswith(";"):
            continue
        text = "".join(stmt)
        name = _statement_table(text, table_to_db)
        is_copy = text.lstrip().upper().startswith("COPY ")
        if name is None:
            if not seen_table:
                preamble.extend(comments + stmt)
            if is_copy:  # COPY of a table we do not know: skip its data block
                in_copy, copy_db = True, None
        else:
            seen_table = True
            db = table_to_db[name]
            keep = db in wanted and not _DROP.search(text)
            if keep:
                emit(db, comments + stmt)
            if is_copy:
                in_copy = True
                copy_db = db if keep else None
        comments, stmt = [], []

    return {db: preamble + lines for db, lines in out.items()}
