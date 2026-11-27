import { TriangleAlert } from "lucide-react";
import type { useBuildChecks } from "@/hooks/api/customApps/useBuildChecks";
import { cn } from "@/libs/shadcn/utils";
import { AdminAsync } from "@/pages/admin/components/AdminAsync";
import { ADMIN_TONE } from "@/pages/admin/components/adminTone";
import type { BuildChecksEnvironment } from "@/types/appBuildChecks";
import { type ChecksTarget, rerunRefusal } from "../buildChecksModel";
import { BuildChecksList } from "./BuildChecksList";
import { BuildHeading } from "./BuildHeading";
import { ChecksProgress } from "./ChecksProgress";

/**
 * The Checks panel for one build: its heading, what "Run again" was told when it was
 * refused, then the run itself: how far it is, the line a stalled attempt gets, and
 * one row per check.
 */
export const BuildChecksBody = ({
  appId,
  target,
  checks,
  environment,
  onShowStaging,
  refused,
  stalled,
  seenRequired
}: {
  appId: string;
  target: ChecksTarget;
  /** The read of the attempt shown. */
  checks: ReturnType<typeof useBuildChecks>;
  environment: BuildChecksEnvironment;
  onShowStaging?: () => void;
  /** What refused "Run again" for this build; `null` when nothing did. */
  refused: Error | null;
  /** The line a stalled attempt gets above its rows. */
  stalled: string | null;
  /** The app holds Promote until a person has seen the changed screens. */
  seenRequired: boolean;
}) => (
  <div className='flex flex-col gap-2'>
    <BuildHeading
      target={target}
      data={checks.data}
      onShowStaging={onShowStaging}
      environment={environment}
    />
    {refused !== null && (
      <p className={cn("text-xs", ADMIN_TONE.danger.text)} data-testid='build-checks-rerun-refused'>
        {rerunRefusal(refused)}
      </p>
    )}
    <AdminAsync query={checks} noun='this build&rsquo;s checks' rows={4}>
      {(data) => (
        <div className='flex flex-col gap-2'>
          <ChecksProgress data={data} />
          {stalled && (
            <p
              role='alert'
              className={cn("flex items-center gap-1.5 text-xs", ADMIN_TONE.danger.text)}
              data-testid='build-checks-stalled'
            >
              <TriangleAlert className='size-3.5 shrink-0' aria-hidden />
              {stalled}
            </p>
          )}
          <BuildChecksList appId={appId} data={data} seenRequired={seenRequired} />
        </div>
      )}
    </AdminAsync>
  </div>
);
