/**
 * How a run's status and a sealed report read in a terminal. `--json` is the
 * server's own body; this is the human/agent summary, and it follows the web
 * page's rules: no effect number before `reported`, a report's `verdict`
 * rather than its raw `significant`, and every stated assumption, every time.
 */

type Obj = Record<string, unknown>;

function say(line: string): void {
  process.stdout.write(`${line}\n`);
}
const obj = (v: unknown): Obj => (v && typeof v === "object" ? (v as Obj) : {});
const list = (v: unknown): unknown[] => (Array.isArray(v) ? v : []);
const str = (v: unknown): string | undefined =>
  typeof v === "string" && v ? v : typeof v === "number" ? String(v) : undefined;

function line(label: string, value: unknown): void {
  const text = str(value);
  if (text !== undefined) say(`${label.padEnd(16)}${text}`);
}

function flipsLine(flips: Obj): string {
  return ["pending", "sent", "confirmed", "unconfirmed", "failed", "cancelled"]
    .map((k) => `${k} ${str(flips[k]) ?? "0"}`)
    .join(", ");
}

function printGuardrails(guardrails: unknown[]): void {
  if (guardrails.length === 0) return;
  say("guardrails");
  for (const g of guardrails.map(obj)) {
    say(
      `  ${str(g.status) ?? "?"}  ${str(g.criteria) ?? ""}${g.detail ? ` — ${str(g.detail)}` : ""}`
    );
  }
}

function printCompliance(compliance: Obj | null): void {
  if (!compliance) return;
  const treated = Number(compliance.n_treated ?? 0);
  if (treated === 0) {
    say(`${"compliance".padEnd(16)}not measured yet`);
    return;
  }
  const share = (n: unknown) => `${(Number(n) * 100).toFixed(1)}%`;
  say(
    `${"compliance".padEnd(16)}treated ${share(compliance.c_treated)}, control leak ${share(compliance.c_control_leak)}${compliance.below_floor ? " — BELOW FLOOR" : ""}`
  );
}

function printRun(run: Obj): void {
  line("run", run.state);
  line("design", run.design);
  line("registered", run.registered_at);
  if (run.retrospective) say(`${"".padEnd(16)}retrospective: built from past data, no lever moved`);
  line("next switch", run.next_switch_on);
  say(`${"flips".padEnd(16)}${flipsLine(obj(run.flips))}`);
  for (const f of list(run.failed_flips).map(obj)) {
    say(`  failed  ${str(f.unit) ?? "?"} — ${str(f.reason) ?? str(f.state) ?? ""}`);
  }
  printCompliance(run.compliance ? obj(run.compliance) : null);
  printGuardrails(list(run.guardrails));
  line("breach", run.guardrail_breach);
  line("abort reason", run.abort_reason);
  line("abort levers", run.abort_levers);
  line("aborted on", run.aborted_on);
  line("checked", run.checked_at);
  line("next check", run.next_check_at);
  if (run.not_advancing) say(`${"".padEnd(16)}NOT ADVANCING — the driver has not moved this run`);
  for (const p of list(run.problems)) say(`  problem  ${str(p) ?? ""}`);
  const estimate = obj(run.estimate);
  line("estimate", estimate.state);
  line("estimate error", estimate.error);
}

/** `GET /run` (`RunStatusBody`), and the body `POST /abort` answers with. */
export function printRunStatus(status: Obj): void {
  line("experiment", status.name);
  line("status", status.status);
  line("refused", status.registration_refusal);
  if (status.retrospective_ready)
    say(`${"".padEnd(16)}its dates are past: it can be analysed retrospectively`);
  line("not yet", status.retrospective_refusal);
  const revisions = list(status.lever_revisions);
  if (revisions.length > 1)
    say(`${"".padEnd(16)}the lever changed mid-run (${revisions.length} revisions)`);
  if (status.run) printRun(obj(status.run));
  else say(`${"run".padEnd(16)}none — not registered`);
}

/** A report's reading — the sealed result's, or an interim's — and its assumptions. */
function printReport(report: Obj, fallbackLabel: string): void {
  say(str(report.label) ?? fallbackLabel);
  line("hypothesis", report.hypothesis);
  line("estimand", report.estimand);
  if (report.refusal) {
    say(`${"no estimate".padEnd(16)}${str(report.refusal)}`);
  } else {
    const headline = obj(report.headline);
    line("result", headline.text);
    line("verdict", headline.verdict);
  }
  const direction = obj(report.direction);
  line("direction", direction.message);
  line("promise", obj(report.promise).message);
  line("substitution", obj(report.substitution).statement);
  const assumptions = list(report.assumptions);
  if (assumptions.length > 0) {
    say("assumptions");
    for (const a of assumptions) say(`  - ${str(a) ?? ""}`);
  }
}

/** `GET /result` (`ResultResponse`), rendered from its `report`. */
export function printResult(result: Obj): void {
  const report = obj(result.report);
  printReport(
    { ...report, assumptions: report.assumptions ?? result.assumptions },
    `${str(result.experiment) ?? ""} — ${str(result.state) ?? ""}`
  );
  const writeBack = obj(result.write_back);
  if (writeBack.available)
    say(`write-back available: oxyc experiments write-back ${str(result.experiment) ?? "<name>"}`);
  else line("write-back", writeBack.reason);
}

/**
 * `GET`/`POST /interim` (`InterimResponse`): the registered estimator over the periods
 * finished so far, on a live or closing run. Its report's label already says it is not
 * the final result; it never carries a total, a projection or a write-back.
 */
export function printInterim(interim: Obj): void {
  const name = str(interim.experiment) ?? "";
  const reading = interim.reading ? obj(interim.reading) : null;
  if (interim.running) say(`${name}: a fresh interim reading is being computed`);
  line("last attempt", interim.error);
  if (!reading) {
    say(
      `${name}: no interim yet — the first is read once a whole period of the first after window has finished; it refreshes daily`
    );
    return;
  }
  printReport(obj(reading.report), `${name} — interim, not final`);
  line("read through", reading.through);
  line("computed", reading.computed_at);
}
