import { TriangleAlert } from "lucide-react";
import React from "react";
import AppSlug from "@/components/ui/AppSlug";
import { Badge } from "@/components/ui/shadcn/badge";
import { Tooltip, TooltipContent, TooltipTrigger } from "@/components/ui/shadcn/tooltip";
import { policyBlockReason } from "@/libs/tokenPolicy";
import type { BlockedOrg, Token } from "@/types/apiToken";
import {
  type Standing,
  sandboxGrantApps,
  summarizeAccess,
  type TokenOwnerVoice
} from "../../../accessSummary";

const STANDING_WORDS: Record<Standing, string> = { platform: "staff", partner: "partner" };

const STANDING_HINTS: Record<Standing, string> = {
  platform: "Carries your Oxygen staff standing",
  partner: "Reaches your client organizations as a partner"
};

/** How many apps are named before the rest become a count: the row is one line. */
const APPS_SHOWN = 2;

/** Orgs whose policy turns this token away. It still works everywhere else. */
const BlockedChip: React.FC<{ blocked: BlockedOrg[] }> = ({ blocked }) => (
  <Tooltip>
    <TooltipTrigger asChild>
      <Badge
        variant='outline'
        className='shrink-0 cursor-default border-destructive/40 text-destructive'
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

interface AppsProps {
  /** Each app as it is shown: its `<org>/<app>` reference, or its name where none is known. */
  apps: { key: string; text: string; isSlug: boolean }[];
  quiet: boolean;
}

/**
 * A sandbox agent token's apps, each by the `<org>/<app>` reference oxyc knows it by. Inline
 * text, so a cell too narrow for them ends in an ellipsis.
 */
const SandboxApps: React.FC<AppsProps> = ({ apps, quiet }) => (
  <>
    {/* A gap separates them for the eye. A screen reader gets the words. */}
    <span className='sr-only'>Sandboxes of </span>
    {apps.slice(0, APPS_SHOWN).map((app, index) => (
      <React.Fragment key={app.key}>
        {index > 0 && <span className='sr-only'>, </span>}
        {app.isSlug ? (
          <AppSlug
            slug={app.text}
            tone={quiet ? "quiet" : "strong"}
            className={index > 0 ? "ml-3 font-normal" : "font-normal"}
          />
        ) : (
          <span className={index > 0 ? "ml-3" : undefined}>{app.text}</span>
        )}
      </React.Fragment>
    ))}
    {apps.length > APPS_SHOWN && (
      <span className='ml-3 text-muted-foreground'>
        <span className='sr-only'>, </span>+{apps.length - APPS_SHOWN} more
      </span>
    )}
  </>
);

interface Props {
  token: Token;
  /** The token no longer works: its access is set back with the rest of its row. */
  quiet?: boolean;
  /**
   * Whose reach the hover speaks of. "you" on the owner's own list; a staff list, read by
   * someone else, passes "its owner".
   */
  owner?: TokenOwnerVoice;
  /**
   * Say the standing after the reach. A list with a column for standing passes `false`, and the
   * label is the reach alone. The hover says the whole of it either way.
   */
  withStanding?: boolean;
}

/**
 * "All access, with staff standing", "3 workspaces in 2 orgs", or a sandbox agent token's apps
 * by reference. One line, cut with an ellipsis where the column is narrow. The tooltip has the
 * whole of it, every app included, then each grant. It is the one hover text: a native `title`
 * beside it would show two at once.
 *
 * An app's reference is its grant's own `org_slug` and `app_slug`. A grant with neither, from an
 * older server, shows the app's name.
 */
const AccessCell: React.FC<Props> = ({
  token,
  quiet = false,
  owner = "you",
  withStanding = true
}) => {
  const summary = summarizeAccess(token, owner);
  const grantApps = token.kind === "sandbox_agent" ? sandboxGrantApps(token.grants ?? []) : [];
  const apps = grantApps.map((app) => ({
    key: app.id ?? app.name,
    text: app.ref ?? app.name,
    isSlug: app.ref !== null
  }));
  const reach =
    apps.length > 0 ? `Sandboxes of ${apps.map((app) => app.text).join(", ")}` : summary.label;
  const standing =
    summary.standing.length > 0
      ? `, with ${summary.standing.map((each) => STANDING_WORDS[each]).join(" and ")} standing`
      : "";

  return (
    <div className='flex min-w-0 items-center gap-1.5' data-testid='account-token-access'>
      <span className='min-w-0 truncate'>
        <Tooltip>
          <TooltipTrigger asChild>
            <span className='cursor-default' data-testid='account-token-access-label'>
              {apps.length > 0 ? <SandboxApps apps={apps} quiet={quiet} /> : summary.label}
            </span>
          </TooltipTrigger>
          <TooltipContent className='max-w-xs' data-testid='account-token-access-tooltip'>
            <p className='font-medium text-xs'>
              {reach}
              {standing}
            </p>
            <ul className='mt-1 flex flex-col gap-1 text-xs'>
              {summary.lines.map((line) => (
                <li key={line}>{line}</li>
              ))}
            </ul>
          </TooltipContent>
        </Tooltip>
        {withStanding && summary.standing.length > 0 && (
          <span className='text-muted-foreground'>
            , with{" "}
            {summary.standing.map((each, index) => (
              <span key={each}>
                {index > 0 && " and "}
                <span title={STANDING_HINTS[each]} data-testid={`account-token-standing-${each}`}>
                  {STANDING_WORDS[each]}
                </span>
              </span>
            ))}{" "}
            standing
          </span>
        )}
      </span>
      {summary.blocked.length > 0 && <BlockedChip blocked={summary.blocked} />}
    </div>
  );
};

export default AccessCell;
