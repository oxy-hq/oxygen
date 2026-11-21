// Every package under sdk/ is published (npm, the public skills repo, the
// create-oxy-app templates), and the Rust server it talks to is private. A
// comment naming a server source file is a dead end for every reader outside
// this repo, and it rots the first time the file moves. Name the contract
// instead: the HTTP route, the wire field, the behaviour.

import { execFileSync } from "node:child_process";
import { readFileSync } from "node:fs";
import { join } from "node:path";
import { describe, expect, it } from "vitest";

const REPO = execFileSync("git", ["rev-parse", "--show-toplevel"], {
  cwd: import.meta.dirname,
  encoding: "utf8"
}).trim();

// A `crates/<…>.rs` path. Built from parts so this file does not match itself.
const PRIVATE_SOURCE = new RegExp(["crates", "/[A-Za-z0-9_./-]+\\.rs\\b"].join(""));

// Files allowed to hold one ON PURPOSE: the skills-repo test feeds a private
// path to the skills publisher to prove it is refused.
const FIXTURES = new Set(["sdk/cli/scripts/skills-repo.test.mjs"]);

const TEXT = /\.(?:[cm]?[jt]sx?|json|md|ya?ml|css|html|sh|example|toml)$/;

describe("sdk sources", () => {
  it("never name a private server source path", () => {
    const tracked = execFileSync("git", ["ls-files", "-z", "sdk"], {
      cwd: REPO,
      encoding: "utf8",
      maxBuffer: 16 * 1024 * 1024
    })
      .split("\0")
      .filter((file) => file && TEXT.test(file) && !FIXTURES.has(file));
    expect(tracked.length).toBeGreaterThan(100);

    const leaks = [];
    for (const file of tracked) {
      const lines = readFileSync(join(REPO, file), "utf8").split("\n");
      lines.forEach((line, i) => {
        const hit = PRIVATE_SOURCE.exec(line);
        if (hit) leaks.push(`${file}:${i + 1} names ${hit[0]}`);
      });
    }
    expect(leaks, "describe the route, wire field or behaviour instead").toEqual([]);
  });
});
