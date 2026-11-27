import { ExternalLink } from "lucide-react";
import { Button } from "@/components/ui/shadcn/button";
import { timeAgo } from "@/libs/utils/date";
import type {
  BuildChecks,
  BuildChecksEnvironment,
  BuildChecksRunner
} from "@/types/appBuildChecks";
import { shortBuildId } from "../../../AppLifecycle/lifecycleCopy";
import { type ChecksTarget, checkedWhen, runnerHref } from "../buildChecksModel";

/** The id the heading shows: the checks' own once they are read, else the target's. */
const headingBuildId = (target: ChecksTarget, data: BuildChecks | undefined) =>
  data?.build.build_id ?? target.build?.build_id ?? target.ref;

/** Whether the environment the attempt ran on serves the build. */
const servedBy = (target: ChecksTarget, environment: BuildChecksEnvironment) =>
  environment === "production" ? (target.build?.is_published ?? false) : target.staging;

/** `· GitHub Actions run #1482`: the runner's label, a link to its run when it has one. */
const RunnerName = ({ runner }: { runner: BuildChecksRunner }) => {
  const href = runnerHref(runner);
  return (
    <span data-testid='build-checks-runner'>
      ·{" "}
      {href ? (
        <a
          href={href}
          target='_blank'
          rel='noreferrer'
          className='inline-flex items-baseline gap-1 font-medium text-primary underline-offset-4 hover:underline'
          data-testid='build-checks-runner-link'
        >
          {runner.label}
          <ExternalLink className='size-3 shrink-0 self-center' aria-hidden />
        </a>
      ) : (
        runner.label
      )}
    </span>
  );
};

/** `9087d7b · on staging · attempt 2 · checked 3 minutes ago · GitHub Actions run #1482`.
 *  The runner is named once one has claimed the attempt, and links to its run when it
 *  has one. For the attempt run on production it says whether production serves the
 *  build instead. */
export const BuildHeading = ({
  target,
  data,
  onShowStaging,
  environment = "staging"
}: {
  target: ChecksTarget;
  data: BuildChecks | undefined;
  onShowStaging?: () => void;
  environment?: BuildChecksEnvironment;
}) => {
  const production = environment === "production";
  const serves = servedBy(target, environment);
  const buildId = headingBuildId(target, data);
  const when = data ? checkedWhen(data) : null;
  const runner = data?.runner;
  return (
    <div
      className='flex flex-wrap items-baseline gap-x-1.5 text-muted-foreground text-xs'
      data-testid='build-checks-build'
    >
      <span className='font-mono text-foreground' title={buildId}>
        {shortBuildId(buildId)}
      </span>
      <span>
        · {serves ? "on" : "not on"} {environment}
      </span>
      {data?.attempt != null && data.attempt > 1 && <span>· attempt {data.attempt}</span>}
      {when && (
        <span title={new Date(when.at).toLocaleString()} data-testid='build-checks-when'>
          · {when.verb} {timeAgo(when.at)}
        </span>
      )}
      {runner && <RunnerName runner={runner} />}
      {!production && !target.staging && onShowStaging && (
        <Button
          variant='link'
          size='sm'
          className='ml-auto h-auto p-0 text-xs'
          onClick={onShowStaging}
          data-testid='build-checks-show-staging'
        >
          Staging&rsquo;s build
        </Button>
      )}
    </div>
  );
};
