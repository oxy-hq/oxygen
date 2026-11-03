import { cn } from "@/libs/shadcn/utils";
import { ADMIN_TONE, type AdminTone } from "@/pages/admin/components/adminTone";
import type { EnvironmentOltpBranch } from "@/services/api/customApps";

const TONE: Record<string, AdminTone> = {
  ready: "ok",
  failed: "danger",
  cutting: "warn",
  migrating: "warn",
  resetting: "warn"
};

/**
 * A sandbox's own OLTP branch: `ready` serves; anything else refuses `ctx.oltp`
 * until it is. A failure carries the server's reason as a tooltip.
 */
export const OltpBranchStatus = ({
  branch,
  name
}: {
  branch: EnvironmentOltpBranch;
  name: string;
}) => (
  <span
    className={cn(ADMIN_TONE[TONE[branch.status] ?? "muted"].text)}
    title={branch.error ?? undefined}
    data-testid={`admin-app-env-oltp-${name}`}
  >
    oltp {branch.mode} {branch.status}
  </span>
);
