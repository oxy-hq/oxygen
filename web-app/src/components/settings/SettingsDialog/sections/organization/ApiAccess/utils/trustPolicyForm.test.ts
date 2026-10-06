import { describe, expect, it } from "vitest";
import type { TrustPolicy } from "@/types/orgApiAccess";
import {
  buildCreateTrustPolicyRequest,
  buildUpdateTrustPolicyRequest,
  EMPTY_TRUST_POLICY_FORM,
  ENVIRONMENT_REQUIRED_MESSAGE,
  environmentRequiredFor,
  environmentState,
  formFromPolicy,
  hasErrors,
  normalizeRepository,
  normalizeWorkflowPath,
  numericIdError,
  refPatternHint,
  repositoryError,
  type TrustPolicyFormState,
  validateTrustPolicyForm,
  workflowPathError
} from "./trustPolicyForm";

const form = (over: Partial<TrustPolicyFormState> = {}): TrustPolicyFormState => ({
  ...EMPTY_TRUST_POLICY_FORM,
  repository: "acme/storefront",
  workflowPath: "release.yml",
  environment: "production",
  access: {
    scope: "selected",
    workspaces: [{ workspace_id: "ws-1", role_ceiling: "member" }],
    appIds: []
  },
  ...over
});

const CREATE = { mode: "create", requireEnvironment: true, needsIds: false } as const;

describe("normalizeRepository", () => {
  it.each([
    ["acme/storefront", "acme/storefront"],
    ["  acme/storefront  ", "acme/storefront"],
    ["https://github.com/acme/storefront", "acme/storefront"],
    ["https://github.com/acme/storefront.git", "acme/storefront"],
    ["git@github.com:acme/storefront.git", "acme/storefront"],
    ["github.com/acme/storefront/", "acme/storefront"]
  ])("reads %s as %s", (raw, expected) => {
    expect(normalizeRepository(raw)).toBe(expected);
  });
});

describe("repositoryError", () => {
  it.each(["acme/storefront", "oxy-hq/oxy.internal", "a/b", "https://github.com/acme/app"])(
    "accepts %s",
    (value) => {
      expect(repositoryError(value)).toBeNull();
    }
  );

  it("asks for a repository when empty", () => {
    expect(repositoryError("  ")).toBe("Enter the repository as owner/repo.");
  });

  it.each([
    "storefront",
    "acme/",
    "/storefront",
    "acme/store front",
    "-acme/app",
    "acme-/app",
    "a/b/c"
  ])("refuses %s", (value) => {
    expect(repositoryError(value)).toMatch(/owner\/repo/);
  });
});

describe("workflow path", () => {
  it("puts a bare file name under .github/workflows", () => {
    expect(normalizeWorkflowPath("release.yml")).toBe(".github/workflows/release.yml");
  });

  it("leaves a full path alone, minus a leading slash", () => {
    expect(normalizeWorkflowPath("/.github/workflows/release.yml")).toBe(
      ".github/workflows/release.yml"
    );
    expect(normalizeWorkflowPath("./.github/workflows/release.yml")).toBe(
      ".github/workflows/release.yml"
    );
  });

  it.each(["release.yml", "deploy.yaml", ".github/workflows/release.yml"])(
    "accepts %s",
    (value) => {
      expect(workflowPathError(value)).toBeNull();
    }
  );

  it("asks for a file when empty", () => {
    expect(workflowPathError("")).toMatch(/Enter the workflow file/);
  });

  it("refuses a file outside .github/workflows", () => {
    expect(workflowPathError("ci/release.yml")).toMatch(/\.github\/workflows/);
  });

  it.each(["release", "release.json", ".github/workflows/nested/release.yml", "my release.yml"])(
    "refuses %s as not one YAML file",
    (value) => {
      expect(workflowPathError(value)).toMatch(/\.yml or \.yaml/);
    }
  );
});

describe("numericIdError", () => {
  it("accepts a positive integer", () => {
    expect(numericIdError("123456789", "repository id")).toBeNull();
    expect(numericIdError(" 42 ", "owner id")).toBeNull();
  });

  it("names the field it is asking for", () => {
    expect(numericIdError("", "owner id")).toBe("Enter the owner id.");
  });

  it.each(["acme", "12.5", "-4", "0", "99999999999999999999"])("refuses %s", (value) => {
    expect(numericIdError(value, "repository id")).toMatch(/is a number/);
  });
});

describe("refPatternHint", () => {
  it("has nothing to say about a full ref, a leading glob or no pattern", () => {
    for (const value of ["", "refs/heads/main", "refs/tags/v*", "*"]) {
      expect(refPatternHint(value)).toBeNull();
    }
  });

  it("warns about a short branch name that can never match", () => {
    expect(refPatternHint("main")).toMatch(/full ref/);
  });
});

describe("environmentState", () => {
  it("is set whenever an environment is named", () => {
    expect(environmentState("production", true)).toBe("set");
    expect(environmentState("production", false)).toBe("set");
  });

  it("is required when empty under a policy that requires one", () => {
    expect(environmentState("  ", true)).toBe("required");
  });

  it("is unprotected when empty and the org allows it", () => {
    expect(environmentState("", false)).toBe("unprotected");
  });
});

describe("validateTrustPolicyForm", () => {
  it("passes a complete form", () => {
    expect(validateTrustPolicyForm(form(), CREATE)).toEqual({});
    expect(hasErrors({})).toBe(false);
  });

  it("reports every problem with an empty form at once", () => {
    const errors = validateTrustPolicyForm(EMPTY_TRUST_POLICY_FORM, CREATE);
    expect(Object.keys(errors).sort()).toEqual([
      "access",
      "environment",
      "repository",
      "workflowPath"
    ]);
    expect(hasErrors(errors)).toBe(true);
  });

  it("refuses a missing environment only when the org requires one", () => {
    const empty = form({ environment: "" });
    expect(validateTrustPolicyForm(empty, CREATE).environment).toBe(ENVIRONMENT_REQUIRED_MESSAGE);
    expect(validateTrustPolicyForm(empty, { ...CREATE, requireEnvironment: false })).toEqual({});
  });

  it("asks for the numeric ids only after the server couldn't resolve them", () => {
    expect(validateTrustPolicyForm(form(), CREATE)).not.toHaveProperty("repositoryId");
    const errors = validateTrustPolicyForm(form(), { ...CREATE, needsIds: true });
    expect(errors.repositoryId).toBeDefined();
    expect(errors.repositoryOwnerId).toBeDefined();
  });

  it("does not validate the repository when editing, since it can't change", () => {
    const errors = validateTrustPolicyForm(form({ repository: "" }), {
      mode: "edit",
      requireEnvironment: true,
      needsIds: true
    });
    expect(errors).toEqual({});
  });

  it("accepts an app-publish grant alone as access", () => {
    const appOnly = form({ access: { scope: "selected", workspaces: [], appIds: ["app-1"] } });
    expect(validateTrustPolicyForm(appOnly, CREATE)).toEqual({});
  });
});

describe("buildCreateTrustPolicyRequest", () => {
  it("normalizes what was typed and leaves the ids for the server to resolve", () => {
    const request = buildCreateTrustPolicyRequest(
      form({
        repository: "https://github.com/acme/storefront.git",
        refPattern: " refs/heads/main "
      }),
      "member",
      { includeIds: false }
    );
    expect(request).toEqual({
      repository: "acme/storefront",
      workflow_path: ".github/workflows/release.yml",
      environment: "production",
      ref_pattern: "refs/heads/main",
      allow_self_hosted: false,
      grants: [{ kind: "workspace", workspace_id: "ws-1", role_ceiling: "member" }]
    });
    expect(request).not.toHaveProperty("repository_id");
  });

  it("sends null, not an empty string, for a blank environment and ref pattern", () => {
    const request = buildCreateTrustPolicyRequest(
      form({ environment: " ", refPattern: "" }),
      "member",
      { includeIds: false }
    );
    expect(request.environment).toBeNull();
    expect(request.ref_pattern).toBeNull();
  });

  it("sends the ids as numbers once they were asked for", () => {
    const request = buildCreateTrustPolicyRequest(
      form({ repositoryId: " 123 ", repositoryOwnerId: "456" }),
      "member",
      { includeIds: true }
    );
    expect(request.repository_id).toBe(123);
    expect(request.repository_owner_id).toBe(456);
  });

  it("keeps self-hosted runners off unless switched on", () => {
    expect(
      buildCreateTrustPolicyRequest(form(), "member", { includeIds: false }).allow_self_hosted
    ).toBe(false);
    expect(
      buildCreateTrustPolicyRequest(form({ allowSelfHosted: true }), "member", {
        includeIds: false
      }).allow_self_hosted
    ).toBe(true);
  });

  it("states an org-wide grant explicitly, at the account's role", () => {
    const request = buildCreateTrustPolicyRequest(
      form({ access: { scope: "org", workspaces: [], appIds: [] } }),
      "admin",
      { includeIds: false }
    );
    expect(request.grants).toEqual([
      { kind: "workspace", workspace_id: null, role_ceiling: "admin" }
    ]);
  });
});

describe("buildUpdateTrustPolicyRequest", () => {
  it("never sends the repository or its ids", () => {
    const request = buildUpdateTrustPolicyRequest(form({ repositoryId: "1" }), "member");
    expect(request).not.toHaveProperty("repository");
    expect(request).not.toHaveProperty("repository_id");
    expect(Object.keys(request).sort()).toEqual([
      "allow_self_hosted",
      "environment",
      "grants",
      "ref_pattern",
      "workflow_path"
    ]);
  });

  it("leaves out an environment that stays empty, so the edit doesn't read as clearing one", () => {
    const empty = form({ environment: "" });
    expect(
      buildUpdateTrustPolicyRequest(empty, "member", { environment: null })
    ).not.toHaveProperty("environment");
    // Clearing one the policy had is said, and the server may refuse it.
    expect(
      buildUpdateTrustPolicyRequest(empty, "member", { environment: "production" })
    ).toHaveProperty("environment", null);
    expect(
      buildUpdateTrustPolicyRequest(form({ environment: "prod" }), "member", { environment: null })
    ).toHaveProperty("environment", "prod");
  });
});

describe("environmentRequiredFor", () => {
  it("binds every create, and an edit only when it could clear an environment", () => {
    expect(environmentRequiredFor(null, true)).toBe(true);
    expect(environmentRequiredFor({ environment: "production" }, true)).toBe(true);
    expect(environmentRequiredFor({ environment: null }, true)).toBe(false);
    expect(environmentRequiredFor(null, false)).toBe(false);
  });
});

describe("formFromPolicy", () => {
  const policy: TrustPolicy = {
    id: "tp-1",
    org_id: "org-1",
    service_account_id: "sa-1",
    provider: "github_actions",
    repository: "acme/storefront",
    repository_id: 123,
    repository_owner_id: 456,
    workflow_path: ".github/workflows/release.yml",
    environment: null,
    ref_pattern: null,
    allow_self_hosted: true,
    grants: [
      {
        id: "g1",
        kind: "app_publish",
        org_id: "org-1",
        org_name: "Acme",
        workspace_id: null,
        workspace_name: null,
        role_ceiling: null,
        app_id: "app-1",
        app_name: "Store Ops",
        revoked_at: null
      }
    ],
    created_by: null,
    created_at: "2026-09-01T00:00:00Z",
    last_used_at: null,
    disabled_at: null
  };

  it("reads nulls back as empty fields", () => {
    const state = formFromPolicy(policy);
    expect(state.environment).toBe("");
    expect(state.refPattern).toBe("");
    expect(state.allowSelfHosted).toBe(true);
  });

  it("saves back what it loaded when nothing was edited", () => {
    expect(buildUpdateTrustPolicyRequest(formFromPolicy(policy), "member")).toEqual({
      workflow_path: ".github/workflows/release.yml",
      environment: null,
      ref_pattern: null,
      allow_self_hosted: true,
      grants: [{ kind: "app_publish", app_id: "app-1" }]
    });
  });
});
