import { Plus, TriangleAlert } from "lucide-react";
import { useEffect, useState } from "react";
import { Button } from "@/components/ui/shadcn/button";
import { useAppEnvironments } from "@/hooks/api/customApps/useAppEnvironments";
import { useAppSecrets } from "@/hooks/api/customApps/useAppSecrets";
import { isForbidden } from "@/hooks/api/httpStatus";
import { AdminAsync } from "@/pages/admin/components/AdminAsync";
import { AdminInfo } from "@/pages/admin/components/AdminInfo";
import { PRODUCTION } from "@/types/appEnvironments";
import { EnvironmentPicker } from "./EnvironmentPicker";
import { environmentLabel, STAGING } from "./environmentLabel";
import { SecretRow } from "./SecretRow";
import { SetSecretDialog } from "./SetSecretDialog";
import { isUnset } from "./secretState";

/**
 * The AppDetail "Secrets" section: every key this app needs, and whether it has
 * one.
 *
 * The list is the union of two things, which is the whole point of the section.
 * What the app's active build **declares** — the `env` block in `oxy-app.json`,
 * plus every function's `webhook.secretVar` — and what is actually **stored**
 * under `apps/<app_id>/`. Before this, the only way to see an app secret was the
 * workspace settings table, where it appeared as a raw `apps/<uuid>/KEY` row
 * among every other project secret, and the only way to *create* one was from
 * inside a function that could not run without it.
 *
 * Each environment has its own: production's by default, or — for staff who
 * may open non-production — staging's or a sandbox's, from the picker. A key
 * an environment holds no value for but reads from another (staging's, in a
 * sandbox; production's, for a `shared` key) says so instead of "Missing".
 *
 * Rows a person has to act on come first and are the only coloured thing here.
 * A healthy key carries no badge at all: badging every row is what stops the
 * broken one from standing out.
 *
 * The environment is the pane's own (its picker) in the dossier, and the console's
 * on `/admin/apps/:org/:app`, where one switcher scopes Functions and Secrets
 * together: there the pane shows no picker of its own, so the two panels cannot be
 * on different environments.
 */
export const Secrets = ({
  appId,
  environment: controlled,
  onEnvironmentChange
}: {
  appId: string;
  /** The console's environment. When given, the pane follows it and shows no picker. */
  environment?: string;
  onEnvironmentChange?: (environment: string) => void;
}) => {
  /** Whose secrets are shown and written: production unless the picker moved. */
  const [own, setOwn] = useState(PRODUCTION);
  const environment = controlled ?? own;
  const setEnvironment = (next: string) =>
    controlled !== undefined ? onEnvironmentChange?.(next) : setOwn(next);
  /** Show only keys with no value here and none to inherit. */
  const [unsetOnly, setUnsetOnly] = useState(false);
  const secrets = useAppSecrets(appId, environment);
  // Which keys production stores, for the rows of another environment: the
  // same query (and cache entry) as the section badge, so no extra request.
  const production = useAppSecrets(appId, PRODUCTION);
  const productionKeys = production.data
    ? new Set(production.data.entries.filter((e) => e.is_set).map((e) => e.key))
    : null;
  /** The key being set: `""` = a new one; a name = that row's key, and whether
   *  it already has a value (a rotation) or not (a first set). */
  const [editing, setEditing] = useState<{ key: string; stored: boolean } | null>(null);
  // Production is the mount's own guard and never refused here; anything else
  // is refused to a viewer oxy-authz does not let open non-production.
  const refused = environment !== PRODUCTION && isForbidden(secrets.error);

  const switchTo = (next: string) => {
    setEditing(null);
    setEnvironment(next);
  };

  // A sandbox deleted while shown: its secrets answer 404 for good, so once the
  // environments list (the picker's query) no longer offers it, being torn down
  // or gone, the pane goes back to production.
  const environments = useAppEnvironments(appId);
  const sandboxGone =
    environment !== PRODUCTION &&
    environment !== STAGING &&
    !!environments.data &&
    !environments.data.some((e) => e.name === environment && e.status !== "deleting");
  // biome-ignore lint/correctness/useExhaustiveDependencies: fires on the sandbox going, not on every render's new setter
  useEffect(() => {
    if (!sandboxGone) return;
    setEditing(null);
    setEnvironment(PRODUCTION);
  }, [sandboxGone]);

  return (
    <div className='flex flex-col gap-2' data-testid='admin-app-secrets'>
      {controlled === undefined && (
        <EnvironmentPicker appId={appId} value={environment} onChange={switchTo} />
      )}

      {refused ? (
        <NonProductionRefused environment={environment} onBack={() => switchTo(PRODUCTION)} />
      ) : (
        <>
          {/* The gate wraps only the list. "Add secret" stays reachable when the fetch
              fails: setting a key is exactly what an operator may be here to do, and the
              declaration list is not needed to do it. */}
          <AdminAsync query={secrets} noun='this app&rsquo;s secrets' rows={2}>
            {(data) => {
              const unset = data.entries.filter((e) => isUnset(e, productionKeys?.has(e.key)));
              const shown = unsetOnly ? unset : data.entries;
              return (
                <>
                  {data.declaration_error && <DeclarationError message={data.declaration_error} />}
                  {/* Only when it narrows something: with nothing unset, or everything
                    unset, the toggle would select the list it is already showing. */}
                  {unset.length > 0 && unset.length < data.entries.length && (
                    <label className='flex items-center gap-1.5 self-start text-muted-foreground text-xs'>
                      <input
                        type='checkbox'
                        checked={unsetOnly}
                        onChange={(e) => setUnsetOnly(e.target.checked)}
                        data-testid='admin-app-secrets-unset-only'
                      />
                      Unset only ({unset.length} of {data.entries.length})
                    </label>
                  )}
                  {data.entries.length === 0 ? (
                    <EmptyState />
                  ) : (
                    <ul className='flex flex-col gap-1.5' data-testid='admin-app-secrets-list'>
                      {shown.map((entry) => (
                        <SecretRow
                          key={entry.key}
                          appId={appId}
                          environment={environment}
                          entry={entry}
                          productionHolds={productionKeys?.has(entry.key)}
                          onSet={() => setEditing({ key: entry.key, stored: entry.is_set })}
                        />
                      ))}
                    </ul>
                  )}
                </>
              );
            }}
          </AdminAsync>

          <div>
            <Button
              type='button'
              variant='outline'
              size='sm'
              className='h-7 gap-1.5 text-xs'
              onClick={() => setEditing({ key: "", stored: false })}
              data-testid='admin-app-secrets-add'
            >
              <Plus className='size-3' />
              Add secret
            </Button>
          </div>
        </>
      )}

      <SetSecretDialog
        appId={appId}
        environment={environment}
        // A named key is locked to its row; `""` is a new one.
        secretKey={editing?.key ?? null}
        stored={editing?.stored ?? false}
        onClose={() => setEditing(null)}
      />
    </div>
  );
};

/**
 * The viewer may not open this app's non-production environments, so the
 * server refused the list (403). Said plainly, in place of the list, rather
 * than as the generic "couldn't load" an outage gets: retrying will not help,
 * and production is one click away.
 */
const NonProductionRefused = ({
  environment,
  onBack
}: {
  environment: string;
  onBack: () => void;
}) => (
  <div
    className='flex flex-col items-start gap-2 border border-border bg-muted/40 px-2.5 py-2 text-xs'
    data-testid='admin-app-secrets-nonprod-refused'
  >
    <p className='text-muted-foreground'>
      {environmentLabel(environment)} secrets are set by Oxy staff who may open this app&rsquo;s
      non-production environments, and you may not. Production&rsquo;s secrets are unaffected.
    </p>
    <Button type='button' variant='outline' size='sm' className='h-6 text-xs' onClick={onBack}>
      Back to production
    </Button>
  </div>
);

/**
 * The app has neither declarations nor stored keys. An empty screen is an
 * invitation to act, so it names both ways forward — declare what the app needs
 * so the next person sees the list, or just set one now.
 */
const EmptyState = () => (
  <p
    className='flex items-center gap-1.5 text-muted-foreground text-xs'
    data-testid='admin-app-secrets-empty'
  >
    No secrets yet.
    <AdminInfo>
      Add an <code>env</code> block to <code>oxy-app.json</code> to list the keys this app expects,
      or set one below. Functions read them as <code>ctx.env.&lt;KEY&gt;</code>.
    </AdminInfo>
  </p>
);

/**
 * The manifest declares an `env` block that would not parse. Reading it
 * leniently keeps the app working, but staying quiet about it would make a typo
 * look exactly like an app that declares nothing — so it is said out loud, here,
 * next to the list it emptied.
 */
const DeclarationError = ({ message }: { message: string }) => (
  <div
    className='flex items-start gap-1.5 border border-warning/40 bg-warning/10 px-2.5 py-2 text-xs'
    data-testid='admin-app-secrets-declaration-error'
  >
    <TriangleAlert className='mt-px size-3 shrink-0 text-warning' />
    <span className='text-muted-foreground'>{message}</span>
  </div>
);

/**
 * The count in the section header, so a collapsed section still says the app is
 * missing something. Reads the same query as the pane — react-query dedupes, so
 * this costs no extra request — rather than threading the number down through
 * the dossier, which does not otherwise know about any section's data.
 *
 * Renders nothing when there is nothing wrong: a "0 missing" badge is noise on
 * every healthy app, and it would sit beside seven other sections that show no
 * badge at all.
 */
export const SecretsBadge = ({
  appId,
  environment = PRODUCTION
}: {
  appId: string;
  /** Whose count. The dossier leaves it at production: its badge answers "is the
   *  live app missing something", and a sandbox someone is still filling in must
   *  not raise it on every viewer's collapsed section, whatever its picker shows.
   *  The console passes its environment, because there the badge sits beside a
   *  scope chip naming that environment and must count the list under it. */
  environment?: string;
}) => {
  const { data } = useAppSecrets(appId, environment);
  if (!data?.missing_required) return null;
  return (
    <span
      className=' bg-destructive/15 px-1.5 py-px font-medium text-[10px] text-destructive tabular-nums tracking-normal'
      data-testid='admin-app-secrets-missing-badge'
    >
      {data.missing_required} missing
    </span>
  );
};
