"""Stream the BIRD zip and question jsonl into the cache, verifying size and sha256."""
import hashlib
import urllib.request
from pathlib import Path

from . import config


def _download(url: str, dest: Path) -> tuple[int, str]:
    tmp = dest.with_suffix(dest.suffix + ".part")
    h = hashlib.sha256()
    n = 0
    req = urllib.request.Request(url, headers={"User-Agent": "oxy-evals-bird"})
    with urllib.request.urlopen(req) as resp, open(tmp, "wb") as out:
        while chunk := resp.read(1 << 20):
            out.write(chunk)
            h.update(chunk)
            n += len(chunk)
    tmp.replace(dest)
    return n, h.hexdigest()


def _sha256(path: Path) -> str:
    h = hashlib.sha256()
    with open(path, "rb") as f:
        while chunk := f.read(1 << 20):
            h.update(chunk)
    return h.hexdigest()


def _fetch_one(url: str, dest: Path, size: int | None, pin: str) -> None:
    if dest.exists() and (size is None or dest.stat().st_size == size):
        digest = _sha256(dest)
        print(f"{dest.name}: cached ({dest.stat().st_size} bytes)")
    else:
        print(f"{dest.name}: downloading {url}")
        n, digest = _download(url, dest)
        if size is not None and n != size:
            dest.unlink()
            raise SystemExit(f"{dest.name}: got {n} bytes, expected {size}")
    print(f"{dest.name}: sha256 {digest}")
    if not pin:
        print(f"{dest.name}: no sha256 pinned yet; pin the value above in config.py")
    elif digest != pin:
        dest.unlink()
        raise SystemExit(f"{dest.name}: sha256 mismatch (expected {pin}); removed")
    else:
        print("sha256 ok")


def fetch() -> None:
    config.CACHE.mkdir(parents=True, exist_ok=True)
    _fetch_one(config.JSONL_URL, config.CACHE / config.JSONL_NAME, None, config.JSONL_SHA256)
    _fetch_one(
        config.ZIP_URL,
        config.CACHE / config.ZIP_NAME,
        config.ZIP_CONTENT_LENGTH,
        config.ZIP_SHA256,
    )
