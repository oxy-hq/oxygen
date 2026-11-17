import type { FunctionInvocation, InvocationCalls } from "@/types/apps";

/**
 * One failed invocation as text an agent, or a colleague, can act on without the
 * console: what ran, where, how it ended, the `ctx.*` calls that failed in it, and
 * the reads that fetch the rest of the evidence.
 *
 * The issue row has had this ("Copy for an agent", `issueBrief`) since issues were
 * grouped. A single failed invocation, the thing an operator is usually looking at
 * after Run now, had nothing: its error had to be selected by hand out of a `<pre>`,
 * and the call trace beside it could not be copied at all.
 *
 * `calls` is the invocation's trace when it has been fetched (the row is open); the
 * brief says when it has not, rather than implying the trace was empty.
 */
export function invocationBrief(
  app: { id: string; org_slug: string; slug: string },
  fn: string,
  iv: FunctionInvocation,
  calls?: InvocationCalls
): string {
  const outcome = iv.result_status === null ? iv.status : `${iv.status}, HTTP ${iv.result_status}`;
  const failedCalls = (calls?.calls ?? []).filter(
    (c) => c.status === "error" || c.status === "refused"
  );
  const lines = [
    `Custom app function failure: ${app.org_slug}/${app.slug}`,
    "",
    `Function:     ${fn}`,
    `Invocation:   ${iv.id}`,
    `Mode:         ${iv.mode}`,
    `Environment:  ${calls?.environment ?? "unknown"}`,
    `Build:        ${calls?.build_id ?? "unknown"}`,
    `Outcome:      ${outcome}${iv.duration_ms != null ? ` after ${iv.duration_ms}ms` : ""}`,
    `At:           ${iv.created_at}`,
    "",
    iv.error ?? describeNoError(iv)
  ];
  if (calls?.recorded) {
    lines.push("", `ctx calls: ${calls.total_calls} made, ${failedCalls.length} failed or refused`);
    for (const c of failedCalls) {
      const what = [c.verb, c.target].filter(Boolean).join(" ");
      lines.push(
        `  #${c.seq} ${c.op}${what ? ` ${what}` : ""}: ${c.status}${c.error_type ? ` (${c.error_type})` : ""}${c.http_status ? `, HTTP ${c.http_status}` : ""} at ${c.start_ms}ms`
      );
    }
  } else if (calls) {
    lines.push("", `ctx calls: no trace (${calls.reason ?? "not recorded"})`);
  } else {
    lines.push("", "ctx calls: not fetched; read them with the second command below");
  }
  lines.push(
    "",
    "Evidence:",
    `  oxyc api "/api/customer-apps/${app.org_slug}/${app.slug}/logs?hours=168&invocation_id=${iv.id}"`,
    `  oxyc api "/api/customer-apps/${app.id}/invocations/${iv.id}/calls"`,
    `  oxyc api "/api/customer-apps/${app.id}/invocations?function=${encodeURIComponent(fn)}&limit=50"`
  );
  return lines.join("\n");
}

/** A failed invocation with no error text: say what is known, as the issue row does. */
function describeNoError(iv: FunctionInvocation): string {
  if (iv.status === "timeout") return "Timed out. No error text is recorded for a timeout.";
  if (iv.result_status !== null && iv.result_status >= 500) {
    return `The handler answered HTTP ${iv.result_status}. No error text is recorded for a response.`;
  }
  if (iv.status === "success") {
    return "The handler returned, and the platform counted the call as failed: it answered 5xx, or a ctx call it made failed and was caught.";
  }
  return `Ended ${iv.status}, with no error text recorded.`;
}
