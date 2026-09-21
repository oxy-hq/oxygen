import { isAxiosError } from "axios";
import { AdminAsync } from "@/pages/admin/components/AdminAsync";

/**
 * The one custom-apps failure an operator can fix without paging anyone.
 *
 * A 403 here means the account is not on the allow list, and saying so — with the env
 * var to add it to — is the difference between a self-service fix and a Slack message.
 * `AdminAsync` cannot do it: axios reports `"Request failed with status code 403"`, and
 * `errorDetail` deliberately strips that shape, so a 403 routed through the kit renders
 * "Couldn't load the custom-app registry." with no status, no detail and a Retry that
 * can never succeed.
 *
 * **This exists because the redesign deleted it.** `main`'s `AdminCustomApps` had a
 * 403-specific branch ahead of the kit; the rewrite routed every failure through
 * `AdminAsync` and the remediation went with it — a capability regression hidden inside
 * a layout change, which is the thing this branch keeps catching in other people's code.
 *
 * It is one component rather than the two near-identical copies that existed before
 * (`ErrorState` here, `GrantsError` in `OxyAccessPanes/shared.tsx`, whose comment
 * promised it matched "the Apps tab's allow-list message" — a message that had ceased to
 * exist). Every other failure still goes through `AdminAsync`, which prints the server's
 * own message and offers the Retry this block never had.
 */
export const isAllowListError = (error: unknown): boolean =>
  isAxiosError(error) && error.response?.status === 403;

export const AllowListError = ({
  error,
  onRetry,
  noun
}: {
  error: unknown;
  onRetry: () => void;
  /** What failed to load, for the generic branch: "apps", "Oxy-access grants". */
  noun: string;
}) =>
  isAllowListError(error) ? (
    <div className='mx-auto max-w-2xl p-6' data-testid='admin-apps-allow-list-error'>
      <div className='rounded-lg border border-destructive/30 bg-destructive/5 p-6 text-center'>
        <p className='font-medium text-destructive text-xs'>
          Your account isn&rsquo;t on the custom-apps allow list.
        </p>
        <p className='mt-2 text-muted-foreground text-xs'>
          Add your email to the oxy backend&rsquo;s{" "}
          <code className='rounded bg-muted px-1 py-0.5 font-mono'>OXY_GLOBAL_ADMINS</code> env var
          (comma-separated) and restart the server, then refresh.
        </p>
      </div>
    </div>
  ) : (
    <AdminAsync
      className='mx-auto max-w-2xl p-6'
      query={{ isError: true, data: undefined, error, refetch: onRetry }}
      noun={noun}
    >
      {() => null}
    </AdminAsync>
  );
