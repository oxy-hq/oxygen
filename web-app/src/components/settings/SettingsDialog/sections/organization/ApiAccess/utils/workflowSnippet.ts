import { serviceAccountHandle } from "./slug";

export interface WorkflowSnippetInput {
  /**
   * The service account's id: what the workflow names the account BY. The
   * exchange takes nothing else — `acme/deployer` can be taken over by whoever
   * gets the slug `acme` after a rename, and an id cannot.
   */
  accountId: string;
  /** With `accountName`, the readable handle written beside the id as a comment. */
  orgSlug: string;
  accountName: string;
  /** `.github/workflows/release.yml` */
  workflowPath: string;
  environment: string | null;
  refPattern: string | null;
  /** The policy carries an app-publish grant, so the example step publishes. */
  publishesApp: boolean;
}

/** A YAML scalar: bare when that is unambiguous, double-quoted otherwise. */
export function yamlScalar(value: string): string {
  return /^[A-Za-z0-9_][A-Za-z0-9_./-]*$/.test(value) ? value : JSON.stringify(value);
}

/** "release.yml" → "Release"; "deploy-prod.yaml" → "Deploy prod". */
export function workflowName(workflowPath: string): string {
  const file = workflowPath.split("/").pop() ?? "";
  const words = file
    .replace(/\.ya?ml$/i, "")
    .replace(/[-_]+/g, " ")
    .trim();
  if (!words) return "Deploy";
  return words.charAt(0).toUpperCase() + words.slice(1);
}

/**
 * The `on:` block that produces a ref the policy's pattern accepts. A branch
 * or tag pattern maps straight onto a push filter; anything else can't be
 * guessed, so the workflow is left to be started by hand.
 */
export function workflowTrigger(refPattern: string | null): string[] {
  const pattern = refPattern?.trim();
  if (!pattern) return ["on:", "  push:", "    branches: [main]"];

  const branch = pattern.match(/^refs\/heads\/(.+)$/);
  if (branch) return ["on:", "  push:", `    branches: [${yamlScalar(branch[1])}]`];

  const tag = pattern.match(/^refs\/tags\/(.+)$/);
  if (tag) return ["on:", "  push:", `    tags: [${yamlScalar(tag[1])}]`];

  return [
    "on:",
    `  workflow_dispatch:   # add the trigger that produces a ref matching ${pattern}`
  ];
}

/** How a job runs oxyc with nothing installed beforehand. */
const OXYC_RUNNER = "npx --yes @oxy-hq/cli";

/**
 * The whole CI story for one trusted-access policy, ready to paste: this
 * account's id and the policy's environment filled in. The account is named
 * by id, with its handle as a trailing comment so the line still reads.
 *
 * oxyc exchanges the job's OIDC token itself, as `oxyc init-ci` writes it by
 * default. The design's `uses: oxy-hq/setup-oxyc@v1` (§6) is not offered: that
 * action has no public repository yet, so a job naming it fails at "Set up job".
 */
export function buildWorkflowSnippet(input: WorkflowSnippetInput): string {
  const handle = serviceAccountHandle(input.orgSlug, input.accountName);
  const environment = input.environment?.trim();
  const command = input.publishesApp
    ? `${OXYC_RUNNER} publish --promote   # or any oxyc command the grants allow`
    : `${OXYC_RUNNER} whoami   # replace with any oxyc command the grants allow`;

  const lines = [
    `# ${input.workflowPath}`,
    `name: ${yamlScalar(workflowName(input.workflowPath))}`,
    ...workflowTrigger(input.refPattern),
    "",
    "permissions: { id-token: write, contents: read }",
    "",
    "jobs:",
    "  deploy:",
    ...(environment
      ? [`    environment: ${yamlScalar(environment)}   # the trust policy requires it`]
      : []),
    "    runs-on: ubuntu-latest",
    "    steps:",
    "      - uses: actions/checkout@v4",
    "      # oxyc trades this job's identity token for a 15-minute Oxygen token",
    "      # and revokes it when the command ends.",
    `      - run: ${command}`,
    "        env:",
    `          OXY_SERVICE_ACCOUNT: ${yamlScalar(input.accountId)}   # ${handle}`,
    '          npm_config_ignore_scripts: "true"   # oxyc\'s dependencies run no install scripts'
  ];
  return `${lines.join("\n")}\n`;
}

/** The line a shell needs to use a freshly minted token with `oxyc`. */
export function exportTokenSnippet(secret: string): string {
  return `export OXY_TOKEN=${secret}`;
}
