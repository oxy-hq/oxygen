/**
 * The scaffold's `validate.yaml`: the customer repo's compile gate, which
 * `oxyc compile` serves since 0.13.0.
 *
 * Like `publish.yaml`, it is YAML no compiler reads, shipped into customer repos
 * and synced over what is there, so the properties that matter are pinned on the
 * parsed document: it compiles through the deployment (no throwaway Postgres, no
 * `oxy migrate`, no psql INSERT), the OIDC permission is the job's own, the pin
 * names a release that has `oxyc compile`, and the branch reaches the shell only
 * through the environment.
 */

import { readFileSync } from "node:fs";
import { dirname, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import { describe, expect, it } from "vitest";
import { parse } from "yaml";

const PACKAGE_ROOT = resolve(dirname(fileURLToPath(import.meta.url)), "..", "..");
const read = (name: string) =>
  readFileSync(resolve(PACKAGE_ROOT, "template", ".github", "workflows", name), "utf8");
const SOURCE = read("validate.yaml");

interface Step {
  name?: string;
  id?: string;
  if?: string;
  run?: string;
}
interface Job {
  permissions?: Record<string, string>;
  environment?: string;
  services?: unknown;
  env?: Record<string, string>;
  steps: Step[];
}
interface Workflow {
  permissions: Record<string, string>;
  env: Record<string, string>;
  jobs: { compile: Job };
}

const workflow = parse(SOURCE) as Workflow;
const { compile } = workflow.jobs;
const expression = (inner: string) => ["$", "{{ ", inner, " }}"].join("");

function version(text: string): [number, number, number] {
  const [major = 0, minor = 0, patch = 0] = text
    .split(".")
    .map((part) => Number.parseInt(part, 10));
  return [major, minor, patch];
}
function newerThan(a: string, b: string): boolean {
  const [left, right] = [version(a), version(b)];
  for (let i = 0; i < 3; i++) {
    if ((left[i] ?? 0) !== (right[i] ?? 0)) return (left[i] ?? 0) > (right[i] ?? 0);
  }
  return false;
}

/** The first release with `oxyc compile`. A pin older than this has no such command. */
const FIRST_RELEASE_WITH_COMPILE = "0.13.0";

describe("the scaffold's validate workflow", () => {
  it("compiles through the deployment: no Postgres service, no oxy migrate, no psql", () => {
    expect(compile.services).toBeUndefined();
    // What the steps RUN (the header comment still names the job it replaced).
    const runs = compile.steps.map((s) => s.run ?? "").join("\n");
    expect(runs).not.toMatch(/\boxy (migrate|compile)\b/);
    expect(runs).not.toMatch(/\bpsql\b/);
    expect(runs).not.toContain("get.oxy.tech");
    expect(compile.env?.OXY_DATABASE_URL).toBeUndefined();
    expect(runs).toMatch(/oxyc compile "\$BRANCH" --workspace "\$OXY_WORKSPACE_ID"/);
    expect(runs).toContain("--wait --json");
  });

  it("holds the OIDC permission in its one job, bound to the environment a trust policy names", () => {
    expect(workflow.permissions).toEqual({ contents: "read" });
    expect(compile.permissions).toEqual({ contents: "read", "id-token": "write" });
    expect(compile.environment).toBe("validate");
    expect(compile.env?.OXY_SERVICE_ACCOUNT).toBe(expression("vars.OXY_SERVICE_ACCOUNT"));
    expect(compile.env?.OXY_WORKSPACE_ID).toBe(expression("vars.OXY_WORKSPACE_ID"));
  });

  it("hands the branch to the shell through the environment, never inline", () => {
    // `github.head_ref` is attacker-chosen text on a pull request.
    expect(compile.env?.BRANCH).toBe(expression("github.head_ref || github.ref_name"));
    for (const step of compile.steps) {
      expect(step.run ?? "", step.name).not.toContain(["$", "{{"].join(""));
    }
  });

  it("skips green, with a notice, until the repo is set up — and every later step is gated", () => {
    const [setup, ...rest] = compile.steps;
    expect(setup?.id).toBe("setup");
    expect(setup?.run).toContain("ready=false");
    for (const step of rest) expect(step.if, step.name).toBe("steps.setup.outputs.ready == 'true'");
  });

  it("pins a CLI that has `oxyc compile`, never one that is not released, and the same as publish's", () => {
    const pin = workflow.env.OXYC_VERSION ?? "";
    const { version: packageVersion } = JSON.parse(
      readFileSync(resolve(PACKAGE_ROOT, "package.json"), "utf8")
    ) as { version: string };
    expect(pin).toMatch(/^\d+\.\d+\.\d+$/);
    expect(newerThan(pin, packageVersion), `pin ${pin} is ahead of ${packageVersion}`).toBe(false);
    expect(newerThan(FIRST_RELEASE_WITH_COMPILE, pin), `pin ${pin} has no oxyc compile`).toBe(
      false
    );
    const publish = parse(read("publish.yaml")) as { env: Record<string, string> };
    expect(publish.env.OXYC_VERSION).toBe(pin);
  });
});
