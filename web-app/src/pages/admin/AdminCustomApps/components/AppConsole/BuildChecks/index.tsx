import { useEffect } from "react";
import { useRerunBuildChecks } from "@/hooks/api/customApps/useBuildChecks";
import { AdminAsync } from "@/pages/admin/components/AdminAsync";
import type { BuildChecksEnvironment } from "@/types/appBuildChecks";
import type { PromotionChecks } from "@/types/apps";
import { ConsolePanel } from "../../ConsolePanel";
import { promotionGate } from "../promotionChecksModel";
import { stalledLine } from "./buildChecksModel";
import { BuildChecksBody } from "./components/BuildChecksBody";
import { EnvironmentSwitch } from "./components/EnvironmentSwitch";
import { RerunButton } from "./components/RerunButton";
import { useShownChecks } from "./useShownChecks";

export { BuildChecksSummary } from "./BuildChecksSummary";

const PANEL_ID = "build-checks";

const INFO =
  'Run on every build staging serves: it validates, its "check": true functions pass, it loads at desk and phone width, it follows the design system. The results inform; they do not hold Promote.';
const INFO_GATED =
  "Run on every build staging serves. Promote waits for the checks this app requires; the rest inform. After a promote the same screens are opened on production.";

/**
 * Releases › a build › Checks: what ran on the build staging serves (or the one
 * `?checks=` names) and what each check found, each row opening to its evidence.
 * The checks run on a runner outside the platform, so a strip above the rows says how
 * far the run is: queued, runner up, checking, done. "Run again" queues a new attempt;
 * a refusal is said under the header, and it is the primary action once a run got no
 * answer.
 *
 * With the org's promote-checks gate off (`promotionChecks` absent or not enabled)
 * the checks inform only and the panel is staging's. With it on, a build production
 * serves also has an attempt run on production after the move; once one exists a
 * switch in the header shows it instead, for the build `?checks=` names or else the
 * live build. That attempt cannot be run again from here.
 */
export const BuildChecksPanel = ({
  appId,
  requested,
  onShowStaging,
  promotionChecks,
  environment = "staging",
  onEnvironment
}: {
  appId: string;
  /** `?checks=`: a build's row id or `build_id`, or `null` for staging's build. */
  requested: string | null;
  onShowStaging: () => void;
  /** The app's gate, off or absent on an org without the flag. */
  promotionChecks?: PromotionChecks | null;
  /** Which attempt was asked for; production is shown only when one exists. */
  environment?: BuildChecksEnvironment;
  onEnvironment?: (environment: BuildChecksEnvironment) => void;
}) => {
  const gate = promotionGate(promotionChecks);
  const { builds, target, checks, offersProduction, shown } = useShownChecks(appId, requested, {
    gated: gate !== null,
    environment
  });
  const rerun = useRerunBuildChecks(appId);

  // Arriving from the Live build card's Checks row: bring the list on screen.
  useEffect(() => {
    if (!requested) return;
    const frame = requestAnimationFrame(() => {
      document
        .querySelector(`[data-testid="apps-console-panel-${PANEL_ID}"]`)
        ?.scrollIntoView?.({ block: "start", behavior: "smooth" });
    });
    return () => cancelAnimationFrame(frame);
  }, [requested]);

  const refused = rerun.isError && rerun.variables === target?.ref ? rerun.error : null;
  // The run got no answer: running it again is the one thing left to do.
  const stalled = stalledLine(checks.data?.stalled);

  return (
    <ConsolePanel
      id={PANEL_ID}
      title='Checks'
      // The switch names the environment once it shows; until then the panel is staging's.
      scope={offersProduction ? undefined : { label: "Staging", warn: true }}
      info={gate ? INFO_GATED : INFO}
      actions={
        <>
          {offersProduction && (
            <EnvironmentSwitch value={shown} onChange={(next) => onEnvironment?.(next)} />
          )}
          {shown === "staging" && (
            <RerunButton
              primary={!!stalled}
              busy={rerun.isPending}
              disabled={!target}
              onClick={() => target && rerun.mutate(target.ref)}
            />
          )}
        </>
      }
    >
      <AdminAsync query={builds} noun='this app&rsquo;s builds' rows={3}>
        {() =>
          target ? (
            <BuildChecksBody
              appId={appId}
              target={target}
              checks={checks}
              environment={shown}
              onShowStaging={requested ? onShowStaging : undefined}
              refused={refused}
              stalled={stalled}
              seenRequired={gate?.seen_required ?? false}
            />
          ) : (
            <p className='text-muted-foreground text-xs' data-testid='build-checks-no-build'>
              {shown === "production" ? "Nothing in production." : "Nothing on staging."}
            </p>
          )
        }
      </AdminAsync>
    </ConsolePanel>
  );
};
