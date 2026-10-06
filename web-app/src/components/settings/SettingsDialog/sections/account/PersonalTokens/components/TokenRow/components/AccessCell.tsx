import { TriangleAlert } from "lucide-react";
import type React from "react";
import { Badge } from "@/components/ui/shadcn/badge";
import { Tooltip, TooltipContent, TooltipTrigger } from "@/components/ui/shadcn/tooltip";
import { policyBlockReason } from "@/libs/tokenPolicy";
import type { BlockedOrg, Token } from "@/types/apiToken";
import { type Standing, summarizeAccess } from "../../../accessSummary";

const STANDING_LABELS: Record<Standing, string> = { platform: "Staff", partner: "Partner" };

const STANDING_HINTS: Record<Standing, string> = {
  platform: "Carries your Oxygen staff standing",
  partner: "Reaches your client organizations as a partner"
};

/** Orgs whose policy turns this token away. It still works everywhere else. */
const BlockedChip: React.FC<{ blocked: BlockedOrg[] }> = ({ blocked }) => (
  <Tooltip>
    <TooltipTrigger asChild>
      <Badge
        variant='outline'
        className='cursor-default border-destructive/40 text-destructive'
        data-testid='account-token-blocked-chip'
      >
        <TriangleAlert aria-hidden />
        Blocked in {blocked.length} org{blocked.length === 1 ? "" : "s"}
      </Badge>
    </TooltipTrigger>
    <TooltipContent className='max-w-xs'>
      <ul className='flex flex-col gap-1 text-xs'>
        {blocked.map((org) => (
          <li key={org.org_id}>
            <span className='font-medium'>{org.org_name}:</span> {policyBlockReason(org.reason)}
          </li>
        ))}
      </ul>
    </TooltipContent>
  </Tooltip>
);

/** "All access" or "3 workspaces in 2 orgs", with standing and warnings. */
const AccessCell: React.FC<{ token: Token }> = ({ token }) => {
  const summary = summarizeAccess(token);
  return (
    <div className='flex flex-wrap items-center gap-1.5' data-testid='account-token-access'>
      <Tooltip>
        <TooltipTrigger asChild>
          <span className='cursor-default' data-testid='account-token-access-label'>
            {summary.label}
          </span>
        </TooltipTrigger>
        <TooltipContent className='max-w-xs'>
          <ul className='flex flex-col gap-1 text-xs'>
            {summary.lines.map((line) => (
              <li key={line}>{line}</li>
            ))}
          </ul>
        </TooltipContent>
      </Tooltip>
      {summary.standing.map((standing) => (
        <Badge
          key={standing}
          variant='outline'
          className='font-normal'
          title={STANDING_HINTS[standing]}
          data-testid={`account-token-standing-${standing}`}
        >
          {STANDING_LABELS[standing]}
        </Badge>
      ))}
      {summary.blocked.length > 0 && <BlockedChip blocked={summary.blocked} />}
    </div>
  );
};

export default AccessCell;
