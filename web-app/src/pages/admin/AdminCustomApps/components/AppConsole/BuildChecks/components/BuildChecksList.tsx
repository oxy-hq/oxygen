import type { BuildChecks } from "@/types/appBuildChecks";
import type { ChecksScope } from "../checksScope";
import { readNote } from "../notesModel";
import { BuildCheckRow } from "./BuildCheckRow";
import { CheckNote } from "./CheckNote";

/** The build's summary when there is one, then one row per check, in the server's
 *  order. A build published before checks existed says so instead of listing nothing. */
export const BuildChecksList = ({
  appId,
  data,
  seenRequired = false
}: {
  appId: string;
  data: BuildChecks;
  /** The app holds Promote until a person has seen the changed screens. */
  seenRequired?: boolean;
}) => {
  const summary = readNote(data.summary);
  const scope: ChecksScope = {
    appId,
    build: data.build.id,
    environment: data.environment,
    seenRequired
  };
  return (
    <div className='flex flex-col gap-2'>
      {summary && <CheckNote note={summary} data-testid='build-checks-summary' />}
      {data.checks.length === 0 ? (
        <p className='text-muted-foreground text-xs' data-testid='build-checks-none'>
          {data.state === "running" ? "Queued." : "Not checked."}
        </p>
      ) : (
        <ul className='flex flex-col' data-testid='build-checks'>
          {data.checks.map((check) => (
            <BuildCheckRow key={check.kind} scope={scope} check={check} stalled={data.stalled} />
          ))}
        </ul>
      )}
    </div>
  );
};
