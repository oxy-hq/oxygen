import type {
  CreateTrustPolicyRequest,
  ServiceAccountRole,
  TrustPolicy,
  UpdateTrustPolicyRequest
} from "@/types/orgApiAccess";
import {
  type AccessDraft,
  accessDraftError,
  buildGrantInputs,
  draftFromGrants,
  NOTHING_SELECTED
} from "./grants";

/** A trusted-access policy as its dialog holds it: every field a string until it is sent. */
export interface TrustPolicyFormState {
  repository: string;
  workflowPath: string;
  environment: string;
  refPattern: string;
  allowSelfHosted: boolean;
  access: AccessDraft;
  /** Only asked for after the server answers `repository_unresolved`. */
  repositoryId: string;
  repositoryOwnerId: string;
}

export const EMPTY_TRUST_POLICY_FORM: TrustPolicyFormState = {
  repository: "",
  workflowPath: "",
  environment: "",
  refPattern: "",
  allowSelfHosted: false,
  access: NOTHING_SELECTED,
  repositoryId: "",
  repositoryOwnerId: ""
};

export function formFromPolicy(policy: TrustPolicy): TrustPolicyFormState {
  return {
    repository: policy.repository,
    workflowPath: policy.workflow_path,
    environment: policy.environment ?? "",
    refPattern: policy.ref_pattern ?? "",
    allowSelfHosted: policy.allow_self_hosted,
    access: draftFromGrants(policy.grants),
    repositoryId: String(policy.repository_id),
    repositoryOwnerId: String(policy.repository_owner_id)
  };
}

const WORKFLOW_DIR = ".github/workflows/";
// GitHub: an owner is 1–39 letters, digits or single hyphens, not hyphen-edged;
// a repository name is letters, digits, dots, hyphens and underscores.
const REPOSITORY = /^[A-Za-z0-9](?:[A-Za-z0-9]|-(?=[A-Za-z0-9])){0,38}\/[A-Za-z0-9._-]{1,100}$/;

/** `owner/repo` out of whatever was pasted: a URL, a `.git` remote, stray slashes. */
export function normalizeRepository(raw: string): string {
  return raw
    .trim()
    .replace(/^(?:https?:\/\/|git@)?github\.com[/:]/i, "")
    .replace(/\.git$/i, "")
    .replace(/^\/+|\/+$/g, "");
}

export function repositoryError(raw: string): string | null {
  const value = normalizeRepository(raw);
  if (!value) return "Enter the repository as owner/repo.";
  if (!REPOSITORY.test(value)) return "Use the form owner/repo, like acme/storefront.";
  return null;
}

/** A bare file name is taken to live where GitHub requires workflows to live. */
export function normalizeWorkflowPath(raw: string): string {
  const value = raw.trim().replace(/^\.?\/+/, "");
  if (!value) return "";
  return value.includes("/") ? value : `${WORKFLOW_DIR}${value}`;
}

export function workflowPathError(raw: string): string | null {
  const value = normalizeWorkflowPath(raw);
  if (!value) return "Enter the workflow file, like release.yml.";
  if (!value.startsWith(WORKFLOW_DIR)) return `A workflow lives under ${WORKFLOW_DIR}`;
  const file = value.slice(WORKFLOW_DIR.length);
  if (file.includes("/") || !/^[^\s/]+\.ya?ml$/i.test(file)) {
    return "Name one .yml or .yaml file, like release.yml.";
  }
  return null;
}

/** GitHub's numeric ids are positive integers; a name pasted here is the usual mistake. */
export function numericIdError(raw: string, what: string): string | null {
  const value = raw.trim();
  if (!value) return `Enter the ${what}.`;
  if (!/^\d+$/.test(value) || Number(value) < 1 || !Number.isSafeInteger(Number(value))) {
    return `The ${what} is a number, like 123456789.`;
  }
  return null;
}

/** GitHub sends refs in full, so a pattern like `main` never matches anything. */
export function refPatternHint(raw: string): string | null {
  const value = raw.trim();
  if (!value || value.startsWith("refs/") || value.startsWith("*")) return null;
  return "GitHub sends the full ref, like refs/heads/main or refs/tags/v*. This pattern may never match.";
}

/**
 * - `set`: an environment is named; the policy is as tight as it gets.
 * - `required`: none is named and the org's policy refuses that.
 * - `unprotected`: none is named and the org allows it — worth a warning,
 *   because any push to the repository can then run the workflow.
 */
export type EnvironmentState = "set" | "required" | "unprotected";

export function environmentState(
  environment: string,
  requireEnvironment: boolean
): EnvironmentState {
  if (environment.trim()) return "set";
  return requireEnvironment ? "required" : "unprotected";
}

export const ENVIRONMENT_REQUIRED_MESSAGE =
  "This organization requires an environment on every trusted-access policy. Name one, or relax the rule under Policy.";
export const ENVIRONMENT_UNPROTECTED_WARNING =
  "Without an environment, anyone who can push to this repository can run the workflow and get a token. Name a GitHub environment with protection rules to prevent that.";

export interface TrustPolicyFormErrors {
  repository?: string;
  workflowPath?: string;
  environment?: string;
  repositoryId?: string;
  repositoryOwnerId?: string;
  access?: string;
}

interface ValidateOptions {
  /** `repository` can't change once a policy exists, so editing skips it. */
  mode: "create" | "edit";
  /** The org's policy, when known. Unknown is treated as "the server decides". */
  requireEnvironment: boolean;
  /** The server couldn't resolve the repository's ids, so they are asked for. */
  needsIds: boolean;
}

export function validateTrustPolicyForm(
  form: TrustPolicyFormState,
  { mode, requireEnvironment, needsIds }: ValidateOptions
): TrustPolicyFormErrors {
  const errors: TrustPolicyFormErrors = {};
  const set = (key: keyof TrustPolicyFormErrors, message: string | null) => {
    if (message) errors[key] = message;
  };

  if (mode === "create") {
    set("repository", repositoryError(form.repository));
    if (needsIds) {
      set("repositoryId", numericIdError(form.repositoryId, "repository id"));
      set("repositoryOwnerId", numericIdError(form.repositoryOwnerId, "owner id"));
    }
  }
  set("workflowPath", workflowPathError(form.workflowPath));
  if (environmentState(form.environment, requireEnvironment) === "required") {
    errors.environment = ENVIRONMENT_REQUIRED_MESSAGE;
  }
  set("access", accessDraftError(form.access, true));
  return errors;
}

export const hasErrors = (errors: TrustPolicyFormErrors): boolean => Object.keys(errors).length > 0;

const orNull = (value: string): string | null => value.trim() || null;

/** The fields a create and an edit share, as the wire wants them. */
function matchRules(form: TrustPolicyFormState, accountRole: ServiceAccountRole) {
  return {
    workflow_path: normalizeWorkflowPath(form.workflowPath),
    environment: orNull(form.environment),
    ref_pattern: orNull(form.refPattern),
    allow_self_hosted: form.allowSelfHosted,
    // A policy always states its grants; there is no server default to lean on.
    grants: buildGrantInputs(form.access, accountRole, { explicitOrgWide: true })
  };
}

export function buildCreateTrustPolicyRequest(
  form: TrustPolicyFormState,
  accountRole: ServiceAccountRole,
  { includeIds }: { includeIds: boolean }
): CreateTrustPolicyRequest {
  return {
    repository: normalizeRepository(form.repository),
    ...matchRules(form, accountRole),
    ...(includeIds
      ? {
          repository_id: Number(form.repositoryId.trim()),
          repository_owner_id: Number(form.repositoryOwnerId.trim())
        }
      : {})
  };
}

/**
 * Whether the org's environment rule binds this form. The server asks it of every create, but
 * of an edit only when the edit clears an environment the policy had: an edit that leaves a
 * policy without one, as it already was, never fails over a rule it didn't touch.
 */
export const environmentRequiredFor = (
  saved: Pick<TrustPolicy, "environment"> | null,
  requireEnvironment: boolean
): boolean => requireEnvironment && (saved === null || saved.environment !== null);

/**
 * Every field, so the edit says what the policy is — except an `environment` that stays empty.
 * Sent as `null` it would read as clearing one, which the org's rule may refuse.
 */
export function buildUpdateTrustPolicyRequest(
  form: TrustPolicyFormState,
  accountRole: ServiceAccountRole,
  saved?: Pick<TrustPolicy, "environment">
): UpdateTrustPolicyRequest {
  const { environment, ...rest } = matchRules(form, accountRole);
  const untouched = environment === null && saved?.environment === null;
  return untouched ? rest : { ...rest, environment };
}
