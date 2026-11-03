import { FlaskConical } from "lucide-react";
import { useState } from "react";
import { Button } from "@/components/ui/shadcn/button";
import { Input } from "@/components/ui/shadcn/input";
import { Skeleton } from "@/components/ui/shadcn/skeleton";
import {
  useAppEnvironments,
  useCreateAppEnvironment,
  useDeleteAppEnvironment
} from "@/hooks/api/customApps/useAppEnvironments";
import { isForbidden } from "@/hooks/api/httpStatus";
import type { AppEnvironment } from "@/services/api/customApps";
import { LifecycleConfirm } from "../../../AppLifecycle";
import { EnvironmentDrawer } from "./components/EnvironmentDrawer";
import { EnvironmentRow } from "./components/EnvironmentRow";

/**
 * The app's environments — production, staging, and each developer's or agent's dev
 * sandbox — from `GET /customer-apps/{id}/environments`.
 *
 * This backend shipped with no UI at all: a sandbox could be created by `oxyc` and
 * torn down by its idle TTL, and nobody could see which existed or delete one early.
 * One list, one input, one verb. Production and staging are listed for the picture
 * but carry no Delete — the server refuses that with a 400, so the control is not
 * drawn rather than drawn and refused.
 */
export const Environments = ({ appId }: { appId: string }) => {
  const { data, isLoading, error } = useAppEnvironments(appId);
  const create = useCreateAppEnvironment(appId);
  const remove = useDeleteAppEnvironment(appId);
  const [name, setName] = useState("");
  const [deleting, setDeleting] = useState<AppEnvironment | null>(null);
  const [opened, setOpened] = useState<string | null>(null);

  if (isLoading) return <Skeleton className='h-20 w-full' />;
  if (isForbidden(error)) {
    return (
      <p className='text-muted-foreground text-xs' data-testid='admin-app-environments-refused'>
        Environments are shown to Oxy staff who may open this app&rsquo;s non-production
        environments, and you may not.
      </p>
    );
  }
  if (error) {
    return (
      <p className='text-destructive text-xs' data-testid='admin-app-env-error'>
        Couldn't load environments.
      </p>
    );
  }

  const submit = (e: React.FormEvent) => {
    e.preventDefault();
    const trimmed = name.trim();
    if (!trimmed) return;
    create.mutate(trimmed, { onSuccess: () => setName("") });
  };

  return (
    <div className='flex flex-col gap-3' data-testid='admin-app-environments'>
      <ul className='flex flex-col divide-y divide-border/50 rounded-md border'>
        {(data ?? []).map((env) => (
          <EnvironmentRow
            key={env.name}
            env={env}
            onDelete={() => setDeleting(env)}
            onDetails={() => setOpened(env.name)}
          />
        ))}
      </ul>

      <form onSubmit={submit} className='flex items-center gap-2'>
        <Input
          value={name}
          onChange={(e) => setName(e.target.value)}
          placeholder='dev-<handle>'
          className='h-7 max-w-xs font-mono text-xs'
          aria-label='Sandbox name'
          data-testid='admin-app-env-name'
        />
        <Button
          type='submit'
          size='sm'
          variant='outline'
          className='h-7 gap-1.5 text-xs'
          disabled={!name.trim() || create.isPending}
          data-testid='admin-app-env-create'
        >
          <FlaskConical className='size-3.5' />
          {create.isPending ? "Creating…" : "New sandbox"}
        </Button>
      </form>

      <EnvironmentDrawer
        appId={appId}
        env={(data ?? []).find((e) => e.name === opened) ?? null}
        onClose={() => setOpened(null)}
      />

      <LifecycleConfirm
        testId='admin-app-env-confirm'
        copy={
          deleting
            ? {
                title: `Delete sandbox ${deleting.name}?`,
                description:
                  "Its build, host and OLTP schema are torn down in the background. Anyone publishing to it gets a fresh sandbox next time.",
                confirm: "Delete"
              }
            : null
        }
        pending={remove.isPending}
        onConfirm={() =>
          deleting && remove.mutate(deleting.name, { onSuccess: () => setDeleting(null) })
        }
        onCancel={() => setDeleting(null)}
      />
    </div>
  );
};
