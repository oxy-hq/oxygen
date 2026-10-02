#!/usr/bin/env python3
"""Fail if a workspace member does not inherit `[workspace.lints]`.

The deny list in the root Cargo.toml only reaches a crate whose manifest says

    [lints]
    workspace = true

Cargo has no way to make that the default, so a new crate that leaves it out
compiles and lints clean while every rule in the list is silently off for it.
This is the check that turns "forgot the two lines" into a red build.

Run from anywhere in the repo: `python3 scripts/ci/verify-workspace-lints.py`.
"""

import json
import os
import subprocess
import sys
import tomllib


def main() -> int:
    metadata = json.loads(
        subprocess.check_output(
            ["cargo", "metadata", "--no-deps", "--format-version", "1"]
        )
    )
    root = metadata["workspace_root"]
    missing = []
    for package in metadata["packages"]:
        manifest = package["manifest_path"]
        with open(manifest, "rb") as f:
            lints = tomllib.load(f).get("lints", {})
        if lints.get("workspace") is not True:
            missing.append(os.path.relpath(manifest, root))

    if missing:
        print("These crates do not inherit the workspace lints:", file=sys.stderr)
        for manifest in sorted(missing):
            print(f"  {manifest}", file=sys.stderr)
        print(
            "\nAdd to each:\n\n    [lints]\n    workspace = true\n",
            file=sys.stderr,
        )
        return 1

    print(f"all {len(metadata['packages'])} workspace crates inherit [workspace.lints]")
    return 0


if __name__ == "__main__":
    sys.exit(main())
