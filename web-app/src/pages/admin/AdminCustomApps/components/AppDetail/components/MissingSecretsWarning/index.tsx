import { TriangleAlert } from "lucide-react";
import { useMissingSecretsForBuild } from "@/hooks/api/customApps/useAppSecrets";
import { cn } from "@/libs/shadcn/utils";
import { PRODUCTION } from "@/types/appEnvironments";
import { environmentLabel } from "../Secrets/environmentLabel";

/**
 * Said before a build goes live: the required keys it declares that the target
 * environment has no value for. A warning, never a gate — a key may be about to
 * be set, or the code may cope without it — but a promote that silently ships a
 * webhook answering 401 to every delivery is the failure this exists to name.
 *
 * Said where no plan is read: a first publish. The
 * console's Promote to production and Roll back confirm on the move's plan instead, where
 * `required_secrets_unset` blocks a promote (as it refuses `oxyc promote`) and a
 * rollback only warns.
 *
 * Reconciled server-side (`GET …/secrets?build=`) against the build being
 * promoted, not the one live now: the new build is what may declare a new key.
 * While that answer is on its way it says so, rather than reading as "nothing
 * missing"; a caller confirming the promote also waits for it (`AppLifecycle`).
 */
export const MissingSecretsWarning = ({
  appId,
  buildId,
  environment = PRODUCTION,
  act = "promote",
  className
}: {
  appId: string;
  /** The `app_builds.id` about to go live; nothing is shown without one. */
  buildId: string | undefined;
  environment?: string;
  /** How the build goes live. Only a promote is refused by `oxyc promote`. */
  act?: "promote" | "rollback";
  className?: string;
}) => {
  const { missing, isLoading } = useMissingSecretsForBuild(appId, buildId, environment);
  if (isLoading) {
    return (
      <p
        className={cn("text-muted-foreground text-xs", className)}
        data-testid='admin-app-missing-secrets-checking'
      >
        Checking required secrets…
      </p>
    );
  }
  if (missing.length === 0) return null;
  return (
    <div
      className={cn(
        "flex items-start gap-1.5 border border-warning/40 bg-warning/10 px-2.5 py-2 text-xs",
        className
      )}
      data-testid='admin-app-missing-secrets-warning'
    >
      <TriangleAlert className='mt-px size-3 shrink-0 text-warning' />
      <span className='text-muted-foreground'>
        This build declares required secrets that {environmentLabel(environment).toLowerCase()} does
        not have: <span className='font-mono text-foreground'>{missing.join(", ")}</span>. Set them
        in Secrets before or right after it goes live.
        {act === "promote" && (
          <>
            {" "}
            <code className='font-mono'>oxyc promote</code> refuses this build until they are set.
          </>
        )}
      </span>
    </div>
  );
};
