import type { Organization } from "@/types/organization";

/** The GitHub org `oxyc new` creates customer repos in (`OXYC_ORG`, default `oxy-hq`). */
const CUSTOMERS_GITHUB_ORG = "oxy-hq";

/** One argument for a POSIX shell: single-quoted, with any `'` closed and escaped. */
export function shellQuote(value: string): string {
  return `'${value.replace(/'/g, `'\\''`)}'`;
}

/** The repo name the commands suggest: `<org slug>-oxy`, the naming `oxyc new` documents. */
export function suggestedRepo(org: Pick<Organization, "slug">): string {
  return `${org.slug}-oxy`;
}

/**
 * The commands that set an org up from a customer repo, in order:
 *
 * 1. `oxyc new` creates the repo on GitHub and renders the template into the local
 *    checkout path (`oxyc path`). It neither initialises git there nor pushes.
 * 2. Commit and push it.
 * 3. Look up the org's GitHub connection and the repo's numeric id, which is what
 *    `POST /orgs/{org_id}/onboarding/github` takes (`GitHubSetupRequest`).
 * 4. Connect the repo to the org.
 *
 * `oxyc api` takes a path relative to `/api`, sends `-f` as strings and `-F` typed,
 * and on a GET sends the fields as query parameters.
 */
export function bootstrapScript(org: Pick<Organization, "id" | "slug" | "name">): string {
  const repo = suggestedRepo(org);
  const orgPath = `orgs/${org.id}`;
  return [
    "# 1. Create the customer's repo from the template",
    `oxyc new ${repo} --display ${shellQuote(org.name)}`,
    `cd "$(oxyc path ${repo})"`,
    `git init -b main && git add -A && git commit -m ${shellQuote(`Set up ${org.name}`)}`,
    `git remote add origin https://github.com/${CUSTOMERS_GITHUB_ORG}/${repo}.git && git push -u origin main`,
    "",
    "# 2. Find the org's GitHub connection, then the repo's id",
    `oxyc api ${orgPath}/github/namespaces --jq '.installations[] | {id, name}'`,
    `oxyc api ${orgPath}/github/repositories -f git_namespace_id=<namespace-id> --jq '.[] | select(.name == "${repo}") | .id'`,
    "",
    `# 3. Connect it to ${org.name}`,
    `oxyc api -X POST ${orgPath}/onboarding/github -f namespace_id=<namespace-id> -F repo_id=<repo-id> -f branch=main`
  ].join("\n");
}

/** A blank start (`POST /orgs/{org_id}/onboarding/new`): a minimal config, no repo. */
export function blankStartCommand(org: Pick<Organization, "id" | "name">): string {
  return `oxyc api -X POST orgs/${org.id}/onboarding/new -f name=${shellQuote(org.name)}`;
}
