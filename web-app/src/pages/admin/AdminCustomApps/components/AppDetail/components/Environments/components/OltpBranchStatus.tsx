import { cn } from "@/libs/shadcn/utils";
import { ADMIN_TONE, type AdminTone } from "@/pages/admin/components/adminTone";
import type { AppEnvironment } from "@/services/api/customApps";
import { oltpRecutQueued } from "../utils";

const TONE: Record<string, AdminTone> = {
  ready: "ok",
  failed: "danger",
  cutting: "warn",
  migrating: "warn",
  resetting: "warn"
};

/**
 * A sandbox's own OLTP branch: `ready` serves; anything else refuses `ctx.oltp`
 * until it is. A re-cut the server queued reads as such, not as the `ready` its
 * old branch still reports, and a failure prints the server's reason, wrapped.
 */
export const OltpBranchStatus = ({ env }: { env: AppEnvironment }) => {
  const branch = env.oltp_branch;
  if (!branch) return null;
  const queued = oltpRecutQueued(env);
  return (
    <>
      <span
        className={cn(ADMIN_TONE[queued ? "warn" : (TONE[branch.status] ?? "muted")].text)}
        data-testid={`admin-app-env-oltp-${env.name}`}
      >
        oltp {branch.mode} {queued ? "re-cut queued" : branch.status}
      </span>
      {branch.status === "failed" && branch.error && (
        <span
          className={cn(
            "basis-full whitespace-pre-wrap break-words font-mono",
            ADMIN_TONE.danger.text
          )}
          data-testid={`admin-app-env-oltp-error-${env.name}`}
        >
          {branch.error}
        </span>
      )}
    </>
  );
};
