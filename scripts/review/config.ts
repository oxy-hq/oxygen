// Every tunable of the PR review pipeline in one place. internal-docs/code-review.md
// explains each choice; change the number there and here together.

import type { Category, Effort, Lens, Severity } from "./src/types.ts";

export { CATEGORIES, SEVERITIES } from "./src/types.ts";

export interface ReleaseRule {
  prefixes: readonly string[];
  authors: readonly string[];
  files: readonly string[];
  versionOnly: readonly string[];
}

export interface RiskArea {
  id: string;
  paths: readonly string[];
}

export const MARKER = "oxy-review";
export const SUMMARY_MARKER = `<!-- ${MARKER}:summary -->`;
export const STATE_VERSION = 2;

// A human applies it to merge past a Blocking or unfinished review. Agents never do.
export const OVERRIDE_LABEL = "review-override";
export const GATE_CONTEXT = "review-gate";
export const RISK_CONTEXT = "high-risk-approval";
export const ROADMAP_CONTEXT = "roadmap";

export interface RoadmapConfig {
  /** The roadmap doc and the summary derived from it (internal-docs/platform-roadmap.md). */
  doc: string;
  summary: string;
  /** A PR touching one of these carries a `Roadmap:` trailer: a line id, or `none` + why. */
  productPaths: readonly string[];
  /** Files that never count as product behaviour, even under `productPaths`. */
  exemptPaths: readonly string[];
  /**
   * Conventional-commit types whose PR title already declares that no behaviour changes,
   * so a missing trailer is not held against them. `feat`, `fix`, `refactor` and `perf`
   * are not here: those are the PRs the roadmap is about.
   */
  noTrailerTypes: readonly string[];
}

// The `roadmap` status (src/roadmap.ts): every PR that changes code says which roadmap
// line it moves, or that it moves none and why. Code is deliberately wide here and
// `none -- <reason>` is one line, so the cost of the rule falls on the PR that changes
// behaviour, not on the one that does not. The automated review's general pass judges
// whether a `none` is honest.
export const ROADMAP: RoadmapConfig = {
  doc: "internal-docs/platform-roadmap.md",
  summary: "internal-docs/platform-roadmap-summary.md",
  productPaths: ["crates/**", "web-app/src/**", "sdk/**"],
  exemptPaths: [
    "crates/workspace-hack/**",
    "crates/test-utils/**",
    "crates/telemetry/**",
    "**/tests/**",
    "**/test/**",
    "**/fixtures/**",
    "**/__snapshots__/**",
    "**/*.test.*",
    "**/*.snap",
    "**/*_test.rs",
    "**/*_tests.rs",
    "**/*.stories.*",
    "**/*.md",
    "**/*.mdx",
    // The scaffolds a customer repo is copied from, and anything's CI, are shipped files
    // but not behaviour of the product. Anchored: `sdk/cli/src/template/` is the CLI's own
    // code and `crates/*/src/templates/` holds the agent instructions it ships.
    "sdk/cli/template/**",
    "sdk/create-oxy-app/templates/**",
    "**/.github/**",
    // Manifests, lockfiles and tool config: a version pin or a build setting is not a
    // feature. Not YAML or TOML by extension: `crates/agentic/builder/knowledge/*.yml` is
    // compiled into the Builder Agent's prompts.
    "**/Cargo.toml",
    "**/Cargo.lock",
    "**/package.json",
    "**/pnpm-lock.yaml",
    "**/*.lock",
    "**/.cargo/**",
    "**/rust-toolchain.toml",
    "**/clippy.toml",
    "**/rustfmt.toml",
    "**/tsconfig*.json",
    "**/biome.json*"
  ],
  noTrailerTypes: ["chore", "docs", "test", "ci", "build", "style"]
};

// The release PRs prepare-release.yaml opens are not reviewed, and the test is what they
// change, not who opened them or what the branch is called: the release app (a Bot) on
// a release/ branch, touching only these files, and in Cargo files only `version = "…"`
// lines. Anything else on that branch, a commit pushed onto it included, gets a review.
export const RELEASE_AUTOMATION: ReleaseRule = {
  prefixes: ["release/"],
  authors: ["oxyhelper"],
  files: ["CHANGELOG.md", "Cargo.lock", "Cargo.toml"],
  versionOnly: ["Cargo.lock", "Cargo.toml"]
};

// The identity the review workflow posts as (GITHUB_TOKEN). Review state is read only
// from its comments.
export const REVIEWER_LOGINS: readonly string[] = ["github-actions"];

// Accounts that are not people, for the metrics' "approved by a person" count only.
// Comments are a different matter: a bot's reply, rating or dispute counts like anyone's.
export const BOT_LOGINS: readonly string[] = [
  "github-actions",
  "claude",
  "dependabot",
  "oxygen-bot",
  "copilot"
];

// Opus 5.5 everywhere unless REVIEW_MODEL says otherwise. Uber's best pairing used a
// different model to grade than to generate; REVIEW_VERIFY_MODEL is that knob.
export const MODELS = {
  finder: process.env.REVIEW_MODEL || "claude-opus-5-5",
  verify: process.env.REVIEW_VERIFY_MODEL || process.env.REVIEW_MODEL || "claude-opus-5-5"
};

export const EFFORT: Record<"general" | "lens" | "verify" | "recheck", Effort> = {
  general: "high",
  lens: "medium",
  verify: "high",
  recheck: "medium"
};

// USD. A pass is started only once its cap plus `overshoot` is reserved against
// `total`: the SDK checks its cap after each model call, so the call in flight lands on
// top (the largest seen was $0.22). The sum of everything that can run is above `total`
// on purpose: lenses that do not fit are skipped and named, not run over budget.
export const BUDGET = {
  total: Number(process.env.REVIEW_BUDGET_USD || 6),
  general: 2.0,
  lens: 0.75,
  verify: 1.0,
  recheck: 0.5,
  overshoot: 0.25
};

// Tokens the model is told it has (the SDK's task budget), so a large diff ends with
// findings and a "not reviewed" list instead of being cut off by the dollar cap.
export const TASK_TOKENS = { general: 400_000, lens: 150_000, verify: 200_000, recheck: 100_000 };

export const MAX_LENSES = 3;
export const MAX_FINDINGS_PER_PASS = 12;
export const MAX_NITS = 3;
export const MAX_INLINE = 8;
export const LARGE_PR_LINES = 1000;
export const PR_BODY_CHARS = 6000;

// Uber's finding: readability and style comments rate worst. They are not posted.
export const SUPPRESSED_CATEGORIES: readonly Category[] = ["style"];
// Minimum finder confidence to keep a finding, by severity. Tune from the weekly report.
export const MIN_CONFIDENCE: Record<Severity, number> = {
  blocking: 0.5,
  should_fix: 0.5,
  nit: 0.7
};

// Not reviewed: lockfiles, generated code, binaries. Everything else is, including
// workflows, dependency manifests, SQL and the Markdown that steers agents.
export const IGNORE: readonly string[] = [
  "**/Cargo.lock",
  "**/pnpm-lock.yaml",
  "**/package-lock.json",
  "**/yarn.lock",
  "**/bun.lock",
  "**/bun.lockb",
  "crates/workspace-hack/**",
  "json-schemas/**",
  "examples/json-schemas/**",
  "oxlint-suppressions.json",
  "**/*.min.js",
  "**/*.min.css",
  "**/*.map",
  "**/*.{png,jpg,jpeg,gif,webp,ico,svg,pdf,mp4,mov,webm,woff,woff2,ttf,otf,zip,gz,tgz,parquet,duckdb,db}"
];

// Paths where a person must approve (the `high-risk-approval` status). Short on
// purpose: everything else is reviewed by the pipeline alone.
export const HIGH_RISK: readonly RiskArea[] = [
  {
    id: "authz",
    paths: [
      "crates/authz/**",
      "crates/auth/**",
      "crates/server-authz/**",
      "crates/app/src/server/authz/**"
    ]
  },
  { id: "migrations", paths: ["crates/migration/**"] },
  { id: "billing", paths: ["crates/billing/**"] },
  { id: "ci", paths: [".github/workflows/**", ".github/actions/**"] },
  // The reviewer and gate themselves, the roadmap parser the gate runs included: a PR
  // must not grade its own homework unseen.
  { id: "reviewer", paths: ["scripts/review/**", "scripts/roadmap/**"] }
];

// Lens patterns are matched against code, not prose: a doc that mentions `Command::new`,
// or this file, which spells out every pattern, must not trigger a lens.
export const PATCH_EXCLUDE: readonly string[] = ["**/*.md", "**/*.mdx", "scripts/review/**"];

const SERVER = ["crates/app/src/server/**", "crates/api-*/**"];

// Focused passes, run when the diff triggers them, highest priority first. `paths`
// match any changed file; `patch` is tested against added lines, only in files
// matching `patchPaths` when given. `prompt` is a file under prompts/.
export const LENSES: readonly Lens[] = [
  {
    id: "authz",
    prompt: "lens-authz.md",
    paths: [
      "crates/authz/**",
      "crates/auth/**",
      "crates/server-authz/**",
      "crates/app/src/server/authz/**",
      "**/role_guards*"
    ],
    patch:
      /\b(enforce_guard|role_guards|is_owner|is_app_admin|app_admins|OXY_OWNER|existing_allow)\b|Ring::|matches!\([^)]*\b(Owner|Admin)\b/
  },
  {
    id: "security",
    prompt: "lens-security.md",
    paths: ["crates/auth/**", "**/oauth/**", "**/secrets/**"],
    patch:
      /\b(unsafe|Command::new|dangerouslySetInnerHTML|innerHTML|eval\(|from_raw_parts|set_cookie|jwt|hmac|signature|presign|csrf|redirect_uri|api_key|password)\b|format!\(\s*"\s*(SELECT|INSERT|UPDATE|DELETE)/i
  },
  {
    id: "data-placement",
    prompt: "lens-data-placement.md",
    paths: ["crates/migration/**", "crates/entity/**"],
    patch: /\b(Table::create|create_table|CREATE TABLE|ctx\.oltp|ctx\.airhouse|ctx\.storage)\b/i
  },
  {
    id: "migrations",
    prompt: "lens-migrations.md",
    paths: ["crates/migration/**", "**/migrations/**", "**/*.sql"],
    patch:
      /\b(ALTER TABLE|DROP (TABLE|COLUMN)|RENAME (COLUMN|TO)|CREATE (UNIQUE )?INDEX|drop_column|rename_column)\b/i
  },
  {
    id: "routes",
    prompt: "lens-routes.md",
    paths: ["crates/app/src/server/router/**", "**/role_manifest*"],
    patch: /\.(route|nest)\(|\bworkspace_path\(|\bresolve_state_dir\(|\bGitClient\b/,
    patchPaths: SERVER
  },
  {
    id: "compile-boundary",
    prompt: "lens-compile-boundary.md",
    paths: ["crates/oxy-compile/**", "crates/workspace-fs/**"],
    patch: /ConfigManager::resolve_|\bread_to_string\(|\bworkspace_path\(\)|\.ya?ml"/,
    patchPaths: [...SERVER, "crates/agentic/**", "crates/project/**"]
  },
  {
    id: "task-spec",
    prompt: "lens-task-spec.md",
    paths: [],
    patch: /\btokio::spawn\b|\bspawn_blocking\b|\binterval\(|\bTaskSpec\b/,
    patchPaths: SERVER
  },
  {
    id: "custom-apps-perf",
    prompt: "lens-custom-apps-perf.md",
    paths: [
      "**/custom_apps_serve.rs",
      "**/projects/query.rs",
      "**/semantic_query.rs",
      "**/custom_apps/**"
    ],
    patch: /customer-apps|Cache-Control/,
    patchPaths: SERVER
  },
  {
    id: "ci",
    prompt: "lens-ci.md",
    paths: [".github/**", "justfile", "scripts/**"]
  },
  {
    id: "agent-guidance",
    prompt: "lens-agent-guidance.md",
    paths: [
      "**/CLAUDE.md",
      "**/AGENTS.md",
      ".claude/**",
      "product-context.md",
      "internal-docs/backend-architecture.md"
    ]
  },
  {
    id: "architecture",
    prompt: "lens-architecture.md",
    paths: ["**/Cargo.toml", "crates/agentic/**"]
  }
];

// Read-only tools. `git` is allowed so a reviewer can see why a line exists. Nothing
// that can print the environment (`jq -n env` could): see src/redact.ts.
export const ALLOWED_TOOLS: readonly string[] = [
  "Read",
  "Grep",
  "Glob",
  "Bash(git diff:*)",
  "Bash(git log:*)",
  "Bash(git show:*)",
  "Bash(git blame:*)",
  "Bash(ast-grep:*)",
  "mcp__code-review-graph"
];
