import { describe, expect, it } from "vitest";
import {
  buildWorkflowSnippet,
  exportTokenSnippet,
  type WorkflowSnippetInput,
  workflowName,
  workflowTrigger,
  yamlScalar
} from "./workflowSnippet";

const ACCOUNT_ID = "3f2504e0-4f89-41d3-9a0c-0305e82c3301";

const input = (over: Partial<WorkflowSnippetInput> = {}): WorkflowSnippetInput => ({
  accountId: ACCOUNT_ID,
  orgSlug: "acme",
  accountName: "deployer",
  workflowPath: ".github/workflows/release.yml",
  environment: "production",
  refPattern: null,
  publishesApp: true,
  ...over
});

describe("buildWorkflowSnippet", () => {
  it("reproduces the design's workflow with the account and environment filled in", () => {
    expect(buildWorkflowSnippet(input())).toBe(
      [
        "# .github/workflows/release.yml",
        "name: Release",
        "on:",
        "  push:",
        "    branches: [main]",
        "",
        "permissions: { id-token: write, contents: read }",
        "",
        "jobs:",
        "  deploy:",
        "    environment: production   # the trust policy requires it",
        "    runs-on: ubuntu-latest",
        "    steps:",
        "      - uses: actions/checkout@v4",
        "      # oxyc trades this job's identity token for a 15-minute Oxygen token",
        "      # and revokes it when the command ends.",
        "      - run: npx --yes @oxy-hq/cli publish --promote   # or any oxyc command the grants allow",
        "        env:",
        `          OXY_SERVICE_ACCOUNT: ${ACCOUNT_ID}   # acme/deployer`,
        '          npm_config_ignore_scripts: "true"   # oxyc\'s dependencies run no install scripts',
        ""
      ].join("\n")
    );
  });

  it("names the account by its id, with <org_slug>/<name> only as a comment", () => {
    const accountId = "9b2c1d3e-5f60-4a7b-8c9d-0e1f2a3b4c5d";
    const snippet = buildWorkflowSnippet(
      input({ accountId, orgSlug: "poke-house", accountName: "nightly" })
    );
    expect(snippet).toContain(`OXY_SERVICE_ACCOUNT: ${accountId}   # poke-house/nightly`);
    // The handle is never the value: the exchange refuses it, because a slug
    // can change hands and an id cannot.
    expect(snippet).not.toMatch(/OXY_SERVICE_ACCOUNT: poke-house\//);
  });

  it("never names the setup-oxyc action, which has no public repository to resolve", () => {
    for (const publishesApp of [true, false]) {
      expect(buildWorkflowSnippet(input({ publishesApp }))).not.toContain("uses: oxy-hq/");
    }
  });

  it("always asks for the id-token permission the exchange needs", () => {
    expect(buildWorkflowSnippet(input())).toContain("id-token: write");
  });

  it("leaves the environment line out when the policy has none", () => {
    for (const environment of [null, "", "   "]) {
      expect(buildWorkflowSnippet(input({ environment }))).not.toContain("environment:");
    }
  });

  it("quotes an environment name that YAML would otherwise misread", () => {
    expect(buildWorkflowSnippet(input({ environment: "prod: eu" }))).toContain(
      'environment: "prod: eu"   # the trust policy requires it'
    );
  });

  it("uses a harmless command when the policy can't publish an app", () => {
    const snippet = buildWorkflowSnippet(input({ publishesApp: false }));
    expect(snippet).toContain("- run: npx --yes @oxy-hq/cli whoami");
    expect(snippet).not.toContain("publish");
  });

  it("triggers on what the policy's ref pattern accepts", () => {
    expect(buildWorkflowSnippet(input({ refPattern: "refs/tags/v*" }))).toContain('tags: ["v*"]');
  });
});

describe("workflowTrigger", () => {
  it("defaults to a push to main", () => {
    expect(workflowTrigger(null)).toEqual(["on:", "  push:", "    branches: [main]"]);
    expect(workflowTrigger("  ")).toEqual(["on:", "  push:", "    branches: [main]"]);
  });

  it("maps a branch pattern onto a branch filter", () => {
    expect(workflowTrigger("refs/heads/main").at(-1)).toBe("    branches: [main]");
    expect(workflowTrigger("refs/heads/release/*").at(-1)).toBe('    branches: ["release/*"]');
  });

  it("maps a tag pattern onto a tag filter", () => {
    expect(workflowTrigger("refs/tags/v1.2.3").at(-1)).toBe("    tags: [v1.2.3]");
  });

  it("falls back to a manual trigger for a pattern it can't translate", () => {
    const lines = workflowTrigger("refs/pull/*/merge");
    expect(lines[1]).toMatch(/^ {2}workflow_dispatch:/);
    expect(lines[1]).toContain("refs/pull/*/merge");
  });
});

describe("workflowName", () => {
  it("reads the name off the file", () => {
    expect(workflowName(".github/workflows/release.yml")).toBe("Release");
    expect(workflowName(".github/workflows/deploy-prod.yaml")).toBe("Deploy prod");
  });

  it("falls back when there is no file name to read", () => {
    expect(workflowName("")).toBe("Deploy");
  });
});

describe("yamlScalar", () => {
  it("leaves a plain word bare", () => {
    expect(yamlScalar("production")).toBe("production");
    expect(yamlScalar("release/2026")).toBe("release/2026");
  });

  it("quotes anything with a glob, a space or a colon", () => {
    expect(yamlScalar("v*")).toBe('"v*"');
    expect(yamlScalar("Deploy prod")).toBe('"Deploy prod"');
    expect(yamlScalar("a: b")).toBe('"a: b"');
  });
});

describe("exportTokenSnippet", () => {
  it("is the line oxyc reads the token from", () => {
    expect(exportTokenSnippet("oxy_sat_abc123")).toBe("export OXY_TOKEN=oxy_sat_abc123");
  });
});
