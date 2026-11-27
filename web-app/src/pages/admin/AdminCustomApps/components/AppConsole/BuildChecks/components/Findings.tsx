import { cn } from "@/libs/shadcn/utils";
import { ADMIN_TONE } from "@/pages/admin/components/adminTone";
import type { BuildCheckFinding } from "@/types/appBuildChecks";
import { findingTone } from "../buildChecksModel";

/** What a check found, every kind alike: its severity, then `where rule: message`. */
export const Findings = ({ findings }: { findings: readonly BuildCheckFinding[] }) => {
  if (findings.length === 0) return null;
  return (
    <ul className='flex flex-col gap-1' data-testid='build-check-findings'>
      {findings.map((f, i) => (
        <li
          // biome-ignore lint/suspicious/noArrayIndexKey: findings have no stable id
          key={i}
          className='flex items-baseline gap-2 text-xs'
          data-testid='build-check-finding'
          data-severity={f.severity}
        >
          <span
            className={cn("w-14 shrink-0 font-medium", ADMIN_TONE[findingTone(f.severity)].text)}
          >
            {f.severity}
          </span>
          <span className='min-w-0 break-words' data-testid='build-check-finding-text'>
            {f.at && <span className='font-mono'>{f.at}</span>}
            {f.at && f.rule && " "}
            {f.rule && <span className='font-mono text-muted-foreground'>{f.rule}</span>}
            {(f.at || f.rule) && ": "}
            {f.message}
          </span>
        </li>
      ))}
    </ul>
  );
};
