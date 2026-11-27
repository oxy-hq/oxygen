import { cn } from "@/libs/shadcn/utils";
import { ADMIN_TONE } from "@/pages/admin/components/adminTone";
import type { BuildCheck, BuildCheckDetail } from "@/types/appBuildChecks";
import { belowFloor } from "../buildChecksModel";
import type { ChecksScope } from "../checksScope";
import { readNotes } from "../notesModel";
import { ChangesEvidence } from "./ChangesEvidence";
import { CheckNote } from "./CheckNote";
import { Findings } from "./Findings";
import { FunctionsEvidence } from "./FunctionsEvidence";
import { LoadsEvidence } from "./LoadsEvidence";
import { ProductionSmokeEvidence } from "./ProductionSmokeEvidence";
import { ScreensEvidence } from "./ScreensEvidence";

/**
 * What an opened check shows: its kind's own evidence, then the findings every kind
 * carries, then any notes. A kind this console does not know shows its findings only.
 */
export const CheckEvidence = ({ scope, check }: { scope: ChecksScope; check: BuildCheck }) => {
  const detail: BuildCheckDetail = check.detail ?? {};
  return (
    <div className='flex flex-col gap-2' data-testid={`build-check-evidence-${check.kind}`}>
      <KindEvidence scope={scope} check={check} detail={detail} />
      <Findings findings={detail.findings ?? []} />
      {readNotes(check.notes).map((note) => (
        <CheckNote
          key={`${note.created_at}:${note.text}`}
          note={note}
          data-testid='build-check-note'
        />
      ))}
    </div>
  );
};

const KindEvidence = ({
  scope,
  check,
  detail
}: {
  scope: ChecksScope;
  check: BuildCheck;
  detail: BuildCheckDetail;
}) => {
  switch (check.kind) {
    case "validation":
      return <ValidationEvidence detail={detail} />;
    case "functions":
      return detail.functions?.length ? (
        <FunctionsEvidence appId={scope.appId} functions={detail.functions} />
      ) : null;
    case "loads":
      return <LoadsEvidence detail={detail} />;
    case "design_system":
      return <DesignSystemEvidence detail={detail} />;
    case "smoke":
      return <ScreensEvidence detail={detail} />;
    case "changes":
      return <ChangesEvidence scope={scope} check={check} detail={detail} />;
    case "production_smoke":
      return <ProductionSmokeEvidence detail={detail} />;
    default:
      return null;
  }
};

const ValidationEvidence = ({ detail }: { detail: BuildCheckDetail }) => {
  if (!detail.validation_status) return null;
  const failed = detail.validation_status === "failed";
  return (
    <div className='flex flex-col gap-1 text-xs' data-testid='build-check-validation'>
      <span>
        Validation{" "}
        <span className={cn("font-medium", failed ? ADMIN_TONE.danger.text : ADMIN_TONE.ok.text)}>
          {detail.validation_status}
        </span>
      </span>
      {detail.validation_detail && (
        <pre
          className={cn(
            "max-h-60 overflow-auto whitespace-pre-wrap break-words rounded bg-muted/40 px-2 py-1 font-mono text-xs",
            failed && ADMIN_TONE.danger.text
          )}
        >
          {detail.validation_detail}
        </pre>
      )}
    </div>
  );
};

/** The SDK the bundle ships against the floor, and how many files the theme guard read;
 *  the guard's own findings follow as `file:line rule: message`. */
const DesignSystemEvidence = ({ detail }: { detail: BuildCheckDetail }) => {
  const below = belowFloor(detail.sdk_version, detail.sdk_floor);
  const files = detail.files ?? [];
  return (
    <div
      className='flex flex-wrap items-baseline gap-x-3 gap-y-1 text-xs'
      data-testid='build-check-sdk'
    >
      <span>
        SDK{" "}
        <span
          className={cn("font-medium font-mono", below && ADMIN_TONE.danger.text)}
          data-testid='build-check-sdk-version'
          data-below-floor={below}
        >
          {detail.sdk_version ?? "not found"}
        </span>
        {detail.sdk_floor && (
          <span className='text-muted-foreground'>
            {" "}
            · floor <span className='font-mono'>{detail.sdk_floor}</span>
          </span>
        )}
      </span>
      {files.length > 0 && (
        <span className='text-muted-foreground' title={files.join("\n")}>
          {files.length} file{files.length === 1 ? "" : "s"} read
        </span>
      )}
    </div>
  );
};
