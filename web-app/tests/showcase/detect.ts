// The no-model filter in front of the planner: which PRs could have something
// to show. Deterministic and free, so it runs before anything is paid for.

export interface PrFacts {
  title: string;
  body: string;
  labels: string[];
  files: string[];
}

export type Detection =
  | { candidate: true; type: "feat" | "fix"; hint?: string }
  | { candidate: false; reason: string };

const SKIP_LABEL = "no-showcase";

/** The squash subject's conventional-commit type, as the release announcement reads it. */
export function commitType(title: string): string | undefined {
  const m = /^([A-Za-z]+)(\([^)]*\))?!?:\s*\S/.exec(title.trim());
  return m?.[1]?.toLowerCase();
}

/** Source the browser renders — not tests, stories or type declarations. */
export function isUiSource(path: string): boolean {
  if (!path.startsWith("web-app/src/")) return false;
  if (!/\.(tsx?|css)$/.test(path)) return false;
  return !/(\.test\.|\.spec\.|\.stories\.|__tests__\/|\.d\.ts$)/.test(path);
}

/**
 * The `## Showcase` section of a PR description, if any: an author's steer on
 * what to show. Runs until the next heading of the same or higher level.
 */
export function showcaseHint(body: string): string | undefined {
  const lines = body.split(/\r?\n/);
  const start = lines.findIndex((l) => /^#{1,3}\s*showcase\s*$/i.test(l.trim()));
  if (start < 0) return undefined;
  const rest = lines.slice(start + 1);
  const end = rest.findIndex((l) => /^#{1,3}\s/.test(l.trim()));
  const text = (end < 0 ? rest : rest.slice(0, end)).join("\n").trim();
  return text || undefined;
}

export function detect(pr: PrFacts): Detection {
  if (pr.labels.includes(SKIP_LABEL)) {
    return { candidate: false, reason: `labelled \`${SKIP_LABEL}\`` };
  }
  const type = commitType(pr.title);
  if (type !== "feat" && type !== "fix") {
    return {
      candidate: false,
      reason: `a \`${type ?? "untyped"}\` change is not announced as new or fixed`
    };
  }
  const hint = showcaseHint(pr.body);
  // An author's `## Showcase` section overrides the path rule: a backend
  // change can surface in a screen it never touched.
  if (hint) return { candidate: true, type, hint };
  if (!pr.files.some(isUiSource)) {
    return { candidate: false, reason: "no web-app screen changed" };
  }
  return { candidate: true, type };
}
