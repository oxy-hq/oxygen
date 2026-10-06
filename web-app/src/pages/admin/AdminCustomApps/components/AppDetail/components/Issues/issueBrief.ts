import type { AppIssue } from "@/services/api/appIssues";

/** The issues the live build has had: the number the section header shows. */
export function liveIssueCount(issues: AppIssue[]): number {
  return issues.filter((issue) => issue.on_live_build).length;
}

/**
 * What the last failure was, in words.
 *
 * The error text when there is one. Three kinds of failure record none, and an
 * empty box under a red row reads as "the console lost it" — so each says what
 * is known instead: a timeout, a 5xx whose status was kept, and a `success`
 * the platform counted as failed (a 5xx whose status was not kept, or a
 * `ctx.*` call the handler caught).
 */
export function describeLastFailure(issue: AppIssue): string {
  const { error, status, result_status } = issue.last;
  if (error) return error;
  if (status === "timeout") return "Timed out. No error text is recorded for a timeout.";
  if (result_status !== null && result_status >= 500) {
    return `The handler answered HTTP ${result_status}. No error text is recorded for a response.`;
  }
  if (status === "success") {
    return "The handler returned, and the platform counted the call as failed: it answered 5xx, or a ctx.* call it made failed and was caught. No error text is recorded for either.";
  }
  return `Ended ${status}, with no error text recorded.`;
}

/**
 * One issue as text an agent — or a colleague — can act on without the console.
 *
 * Everything the row shows, plus the two reads that fetch the evidence: the
 * last failing invocation's log lines, and the function's recent invocations.
 * Oxy builds these apps, so the reader of an issue is usually whoever is about
 * to open the app's repository; this is the hand-off.
 */
export function issueBrief(
  app: { id: string; org_slug: string; slug: string },
  issue: AppIssue,
  windowDays: number
): string {
  const { last } = issue;
  const outcome =
    last.result_status === null ? last.status : `${last.status}, HTTP ${last.result_status}`;
  const liveBuild = issue.on_live_build
    ? "has had it"
    : `has not had it (last seen on build ${last.build_id ?? "unknown"})`;
  const builds = issue.builds === 1 ? "1 build" : `${issue.builds} builds`;
  return [
    `Custom app issue: ${app.org_slug}/${app.slug}`,
    "",
    `Function:      ${issue.function_name}`,
    `Fingerprint:   ${issue.fingerprint}`,
    `Occurrences:   ${issue.occurrences} in the last ${windowDays} days, on ${builds}`,
    `First seen:    ${issue.first_seen} (within that window)`,
    `Last seen:     ${issue.last_seen}`,
    `Live build:    ${liveBuild}`,
    `Last failure:  ${outcome}, invocation ${last.invocation_id}, build ${last.build_id ?? "unknown"}`,
    "",
    describeLastFailure(issue),
    "",
    "Evidence:",
    `  oxyc api "/api/customer-apps/${app.org_slug}/${app.slug}/logs?hours=168&invocation_id=${last.invocation_id}"`,
    `  oxyc api "/api/admin/apps/${app.id}/invocations?function=${encodeURIComponent(issue.function_name)}&limit=50"`
  ].join("\n");
}
