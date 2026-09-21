import { Link } from "react-router-dom";
import { usePublishTokens } from "@/hooks/api/publishTokens/usePublishTokens";
import ROUTES from "@/libs/utils/routes";
import { CommandSnippet } from "@/pages/admin/components/CommandSnippet";
import type { CustomApp } from "@/types/apps";
import { ConsolePanel, PanelRow } from "../ConsolePanel";

/**
 * How a repo gets to publish this app — Q6.
 *
 * Publish tokens were a top-level tab, which put "machine auth for CI" beside "the apps"
 * as though it were a peer of them. It is a step in shipping *an* app, so it belongs on
 * the app, with the command already filled in for this org and slug — the thing an
 * operator would otherwise assemble by hand from two places.
 *
 * Tokens themselves are deployment-wide (one token can publish any app the grant
 * reaches), so the count is context here and the count **links to** the management
 * surface. That link is the whole reason this sentence is true: the route
 * (`/admin/publish-tokens`) never moved, but its only *path* was the tab this direction
 * deleted, and `adminNav` deliberately carries no entry for it precisely because it was
 * a tab — so the rail and the ⌘K palette, which both read `ADMIN_NAV`, had nothing
 * either. For one commit, minting or revoking a CI token was a type-the-URL operation.
 *
 * The module-graph walk this branch relies on cannot see that: `AdminPublishTokens` is
 * still routed in `App.tsx`, so it is reachable as a *module* while being unreachable as
 * a *surface*. Routes are the thing to check by hand.
 *
 * What this panel adds beyond the link is the *specific* next command for *this* app.
 */
export const PublishCiPanel = ({ app }: { app: CustomApp }) => {
  const tokens = usePublishTokens();
  const active = tokens.data?.length ?? null;

  return (
    <ConsolePanel id='ci' title='Publishing & CI' question='Q6 — let a repo publish'>
      <div className='flex flex-col gap-2'>
        <PanelRow label='Publish tokens' mono data-testid='apps-console-ci-tokens'>
          {/* Only a link once there is a count to stand behind. While the query is in
              flight or failed, `—` stays plain text: an underlined em-dash invites a
              click on what reads as "no data". */}
          {active === null ? (
            "—"
          ) : (
            <Link
              to={ROUTES.ADMIN.PUBLISH_TOKENS}
              className='underline underline-offset-2 hover:text-foreground'
              data-testid='apps-console-ci-tokens-link'
            >
              {active === 0 ? "none yet" : `${active} active`}
            </Link>
          )}
        </PanelRow>

        <div className='space-y-1.5'>
          <p className='text-muted-foreground text-xs'>
            Trusted publishing via GitHub OIDC is recommended — it stores no long-lived secret. Run
            this in the app&rsquo;s repo:
          </p>
          <div data-testid='apps-console-ci-command'>
            <CommandSnippet command={`oxyc init-ci --app ${app.org_slug}/${app.slug}`} />
          </div>
          <p className='text-muted-foreground text-xs'>
            A token secret is the fallback where OIDC is not available.
          </p>
        </div>
      </div>
    </ConsolePanel>
  );
};
