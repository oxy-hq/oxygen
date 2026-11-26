import { AlertTriangle, CheckCircle2, HelpCircle } from "lucide-react";
import type { AirwayDriftReport, AirwayInstalledScope } from "@/services/api/airwayConfig";
import { DEPLOYMENT_FIELDS } from "./fields";

function labelFor(key: string): string {
  return DEPLOYMENT_FIELDS.find((f) => f.key === key)?.label ?? key;
}

function processLabel(scope: AirwayInstalledScope): string {
  const where = scope.hostname ? `${scope.hostname} (pid ${scope.pid})` : `pid ${scope.pid}`;
  return `${where}, OXY_ROLE=${scope.process_role}`;
}

/**
 * The one thing this region exists to say: **a save here is not live.**
 *
 * airway installs the deployment tier into a process-wide `OnceLock` at worker
 * startup and refuses every later call, so an edit changes what the *next*
 * process installs and nothing about the ones running. Three states, and the
 * third is the one that has to resist being rounded up to the first:
 *
 * - `drifted` — configured and installed differ. Names the settings and asks
 *   for a restart.
 * - `in_sync` — this process installed exactly what is configured.
 * - `unknown` — nothing was observed, so nothing is claimed. Rendered as a
 *   dashed, neutral panel, never as a tick.
 *
 * The `unknown` copy has to track where the install happens. It once explained
 * a missing install in terms of *runs* — true when `install_once` sat at the
 * top of `run_pipeline`. It no longer is: every oxy process that can build an
 * airway connector installs at boot (`crates/app/src/airway_boot.rs`), under
 * every `OXY_ROLE`, so a replica running this build and reporting no install
 * did not merely skip a run — its boot install did not succeed, and that is a
 * `warn` in its log an operator should go and read.
 *
 * What it must still not claim is that drift is impossible. `installed()` is
 * per-process by construction, this is one replica's answer, and a rolling
 * deploy is exactly when the replicas disagree — so the last sentence stays:
 * nothing is asserted about any other process.
 */
export function DeploymentStateBanner({
  drift,
  scope
}: {
  drift: AirwayDriftReport;
  scope: AirwayInstalledScope;
}) {
  if (drift.status === "drifted") {
    return (
      <div
        className='flex items-start gap-2 border border-warning/40 bg-warning/10 p-3'
        data-testid='admin-airway-deployment-banner-drifted'
      >
        <AlertTriangle className='mt-0.5 size-3.5 shrink-0 text-warning' />
        <div className='space-y-1 text-xs'>
          <p className='font-medium text-foreground'>
            Saved, but not in force — restart the airway worker to apply
          </p>
          <p className='text-muted-foreground'>
            {drift.fields.length === 1 ? "This setting differs" : "These settings differ"} from what{" "}
            {processLabel(scope)} installed at startup:{" "}
            <span className='text-foreground'>{drift.fields.map(labelFor).join(", ")}</span>. airway
            installs this tier once per process and refuses every later call, so the stored values
            take effect only on a fresh process.
          </p>
        </div>
      </div>
    );
  }

  if (drift.status === "in_sync") {
    return (
      <div
        className='flex items-start gap-2 border border-border/60 bg-muted/30 p-3'
        data-testid='admin-airway-deployment-banner-in-sync'
      >
        <CheckCircle2 className='mt-0.5 size-3.5 shrink-0 text-muted-foreground' />
        <p className='text-muted-foreground text-xs'>
          {processLabel(scope)} installed exactly what is stored below. Any edit needs a worker
          restart before it applies.
        </p>
      </div>
    );
  }

  const invalid = drift.reason === "configured_values_invalid";
  return (
    <div
      className='flex items-start gap-2 border border-border/60 border-dashed bg-muted/30 p-3'
      data-testid={
        invalid
          ? "admin-airway-deployment-banner-invalid"
          : "admin-airway-deployment-banner-unobserved"
      }
    >
      <HelpCircle className='mt-0.5 size-3.5 shrink-0 text-muted-foreground' />
      <div className='space-y-1 text-xs'>
        <p className='font-medium text-foreground'>
          {invalid ? "Stored values are not installable" : "This process installed no tier"}
        </p>
        <p className='text-muted-foreground'>
          {invalid ? (
            <>
              airway refuses at least one stored value, so the next worker restart will fail on it
              rather than pick it up. Fix the field below and save again.
            </>
          ) : (
            <>
              {processLabel(scope)} answered this request and resolved no airway deployment config.
              Every oxy process that can build an airway connector installs this tier at boot, under
              every role — so on a replica running this build, that means the boot install did not
              succeed: the database was unreachable at startup, or airway refused one of the values
              below (which is not reported separately while nothing is installed). It was logged as
              a warning by that process and nowhere else.
              {scope.process_runs_airway
                ? " This node drains airway runs, so the next run here re-reads the row and either installs it or fails with the reason on its event stream."
                : " A node in this role drains no airway runs; here the tier governs the connectors built outside one — source discovery, policy preview — which stay on airway's built-in settings until this process restarts."}{" "}
              What any other process installed is not visible from here, so no drift is claimed
              either way.
            </>
          )}
        </p>
      </div>
    </div>
  );
}
