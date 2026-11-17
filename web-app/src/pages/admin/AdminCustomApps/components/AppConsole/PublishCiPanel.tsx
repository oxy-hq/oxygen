import { Link } from "react-router-dom";
import { usePublishTokens } from "@/hooks/api/publishTokens/usePublishTokens";
import ROUTES from "@/libs/utils/routes";
import { CommandSnippet } from "@/pages/admin/components/CommandSnippet";
import type { CustomApp } from "@/types/apps";
import { ConsolePanel, PanelRow } from "../ConsolePanel";
import { PromotionRule } from "./PromotionRule";
import { gateLine, productionGates } from "./productionGates";

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
 * The module-graph walk this branch relies on cannot see that: `AdminPublishTokens` was
 * still routed in `App.tsx`, so it was reachable as a *module* while being unreachable as
 * a *surface*. Routes are the thing to check by hand. Publish tokens now live on the
 * Tokens page (`/admin/tokens?kind=publish`), which does have a rail entry; this link
 * opens it on this kind.
 *
 * What this panel adds beyond the link is the *specific* next command for *this* app.
 */
export const PublishCiPanel = ({ app }: { app: CustomApp }) => {
  const tokens = usePublishTokens();
  const active = tokens.data?.length ?? null;
  const gates = productionGates(app.require_second_person_to_promote ?? false);

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
              to={`${ROUTES.ADMIN.TOKENS}?kind=publish`}
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

        {/* Where a publish goes and what refuses a move to production, stated where CI
            is set up. A publish lands on staging. `--promote` (`channel=published`, which
            a publish token may send) moves production too, in the same request. All
            three paths below run the blockers in `custom_apps_promote/blockers.rs`; a
            promoting publish runs them before anything of it lands
            (`custom_apps_publish/production_gate.rs`), so a refusal leaves staging where
            it was too. The list is `productionGates`, which is tested. */}
        <div
          className='space-y-1 border-border/60 border-t pt-2'
          data-testid='apps-console-ci-gates'
        >
          <p className='text-muted-foreground text-xs'>
            A publish lands on <span className='text-foreground'>staging</span>. With{" "}
            <code className='font-mono'>--promote</code> it moves production too, straight from CI.
            Production also moves by Promote to production or a rollback.
          </p>
          <p className='text-muted-foreground text-xs'>All three refuse a build that:</p>
          <ul className='list-disc space-y-0.5 pl-4 text-muted-foreground text-xs'>
            {gates.map((gate) => (
              <li key={gate.code} data-testid={`apps-console-ci-gate-${gate.code}`}>
                {gateLine(gate)}
              </li>
            ))}
          </ul>
          <p className='text-muted-foreground text-xs'>
            A refused promoting publish publishes nothing, staging included.
          </p>
        </div>

        <PromotionRule app={app} />
      </div>
    </ConsolePanel>
  );
};
