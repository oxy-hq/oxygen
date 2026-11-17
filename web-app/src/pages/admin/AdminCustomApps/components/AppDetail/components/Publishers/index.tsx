import { Plus, Trash2 } from "lucide-react";
import { useState } from "react";
import { useForm } from "react-hook-form";
import { Button } from "@/components/ui/shadcn/button";
import { FieldError } from "@/components/ui/shadcn/field";
import { Input } from "@/components/ui/shadcn/input";
import { Label } from "@/components/ui/shadcn/label";
import { Skeleton } from "@/components/ui/shadcn/skeleton";
import {
  useAppPublishers,
  useDeleteAppPublisher,
  useRegisterAppPublisher
} from "@/hooks/api/customApps/useAppPublishers";
import type { RegisterPublisherRequest } from "@/services/api/customApps";
import { relativeTime } from "../Activity/relativeTime";

/**
 * The GitHub workflows trusted to publish this app over OIDC —
 * `GET|POST /customer-apps/{id}/publishers`, `DELETE …/{publisher_id}`.
 *
 * `oxyc init-ci` registers one from the repo; this is where an operator sees what is
 * registered, removes a workflow a repo no longer runs, and registers one by hand
 * when the CLI is not to hand. A publisher is identified by the repo owner's numeric
 * GitHub id, not its name, so a renamed org keeps publishing and a squatted name does
 * not.
 */
export const Publishers = ({ appId }: { appId: string }) => {
  const { data, isLoading, error } = useAppPublishers(appId);
  const remove = useDeleteAppPublisher(appId);
  const [adding, setAdding] = useState(false);

  if (isLoading) return <Skeleton className='h-16 w-full' />;
  if (error) {
    return (
      <p className='text-destructive text-xs' data-testid='admin-app-publishers-error'>
        Couldn't load trusted publishers.
      </p>
    );
  }

  return (
    <div className='flex flex-col gap-3' data-testid='admin-app-publishers'>
      {data && data.length > 0 ? (
        <ul className='flex flex-col divide-y divide-border/50 rounded-md border'>
          {data.map((p) => (
            <li
              key={p.id}
              className='flex items-center gap-3 px-3 py-2 text-xs'
              data-testid={`admin-app-publisher-${p.id}`}
            >
              <div className='flex min-w-0 flex-1 flex-col gap-0.5'>
                <span className='truncate font-medium font-mono'>
                  {p.repo_owner}/{p.repo_name}
                </span>
                <span className='truncate text-muted-foreground'>
                  {p.workflow_ref}
                  {p.environment ? ` · env ${p.environment}` : ""} · added{" "}
                  {relativeTime(p.created_at)}
                </span>
              </div>
              <Button
                variant='ghost'
                size='sm'
                className='h-6 gap-1 px-1.5 text-[11px] text-muted-foreground'
                disabled={remove.isPending}
                onClick={() => remove.mutate(p.id)}
                data-testid={`admin-app-publisher-remove-${p.id}`}
              >
                <Trash2 className='size-3' />
                Remove
              </Button>
            </li>
          ))}
        </ul>
      ) : (
        <p className='text-muted-foreground text-xs' data-testid='admin-app-publishers-empty'>
          No workflow can publish this app yet. Run <code className='font-mono'>oxyc init-ci</code>{" "}
          in its repo, or register one below.
        </p>
      )}

      {adding ? (
        <RegisterForm appId={appId} onDone={() => setAdding(false)} />
      ) : (
        <Button
          variant='outline'
          size='sm'
          className='h-7 w-fit gap-1.5 text-xs'
          onClick={() => setAdding(true)}
          data-testid='admin-app-publisher-add'
        >
          <Plus className='size-3.5' />
          Register a workflow
        </Button>
      )}
    </div>
  );
};

type FormValues = {
  repo_owner: string;
  repo_owner_id: string;
  repo_name: string;
  workflow_ref: string;
  environment: string;
};

const RegisterForm = ({ appId, onDone }: { appId: string; onDone: () => void }) => {
  const register_ = useRegisterAppPublisher(appId);
  const {
    register,
    handleSubmit,
    formState: { errors }
  } = useForm<FormValues>({
    // What `oxyc init-ci` generates, and what the matcher compares against: a bare
    // workflow path. `claims.workflow_path()` strips the `@ref` from the token's
    // `job_workflow_ref` before comparing, so a stored `@ref` can never match.
    defaultValues: {
      workflow_ref: ".github/workflows/oxy-publish.yml",
      environment: "oxy-publish"
    }
  });

  const onSubmit = (v: FormValues) => {
    const body: RegisterPublisherRequest = {
      repo_owner: v.repo_owner.trim(),
      repo_owner_id: Number(v.repo_owner_id),
      repo_name: v.repo_name.trim(),
      workflow_ref: v.workflow_ref.trim(),
      environment: v.environment.trim()
    };
    register_.mutate(body, { onSuccess: onDone });
  };

  return (
    <form
      onSubmit={handleSubmit(onSubmit)}
      className='flex flex-col gap-2 rounded-md border p-3'
      data-testid='admin-app-publisher-form'
    >
      <div className='grid grid-cols-2 gap-2'>
        <Field id='publisher-owner' label='Repo owner' error={errors.repo_owner?.message}>
          <Input
            id='publisher-owner'
            placeholder='oxy-hq'
            className='h-7 font-mono text-xs'
            data-testid='admin-app-publisher-owner'
            {...register("repo_owner", { required: "Required" })}
          />
        </Field>
        <Field
          id='publisher-owner-id'
          label='Owner id (numeric)'
          error={errors.repo_owner_id?.message}
        >
          <Input
            id='publisher-owner-id'
            placeholder='123456'
            inputMode='numeric'
            className='h-7 font-mono text-xs'
            data-testid='admin-app-publisher-owner-id'
            {...register("repo_owner_id", {
              required: "Required",
              validate: (v) => /^\d+$/.test(v.trim()) || "GitHub's numeric owner id"
            })}
          />
        </Field>
        <Field id='publisher-repo' label='Repo' error={errors.repo_name?.message}>
          <Input
            id='publisher-repo'
            placeholder='customer-apps'
            className='h-7 font-mono text-xs'
            data-testid='admin-app-publisher-repo'
            {...register("repo_name", { required: "Required" })}
          />
        </Field>
        {/* Required, not optional: the verifier rejects a token with no
            `environment` claim outright and otherwise compares it to this value,
            so a blank here registers a publisher that can never publish. */}
        <Field
          id='publisher-environment'
          label='GitHub environment'
          error={errors.environment?.message}
        >
          <Input
            id='publisher-environment'
            placeholder='oxy-publish'
            className='h-7 font-mono text-xs'
            data-testid='admin-app-publisher-environment'
            {...register("environment", {
              required: "Required — the environment the publish job runs in"
            })}
          />
          {/* Oxy has no approval step of its own before a publish; GitHub's environment
              protection is where one goes, and this field is what ties the two. The
              token's git ref is not compared (see the workflow field), so branch limits
              belong there too. */}
          <span
            className='text-muted-foreground text-xs'
            data-testid='admin-app-publisher-environment-help'
          >
            To require a reviewer or limit which branches publish, protect this environment in the
            repo&rsquo;s GitHub settings.
          </span>
        </Field>
      </div>
      <Field id='publisher-workflow' label='Workflow path' error={errors.workflow_ref?.message}>
        <Input
          id='publisher-workflow'
          className='h-7 font-mono text-xs'
          data-testid='admin-app-publisher-workflow'
          {...register("workflow_ref", {
            required: "Required",
            validate: (v) =>
              !v.includes("@") || "Path only, no @ref — the token's ref is not compared"
          })}
        />
      </Field>
      <div className='flex justify-end gap-2'>
        <Button
          type='button'
          variant='ghost'
          size='sm'
          className='h-7 text-xs'
          onClick={onDone}
          data-testid='admin-app-publisher-cancel'
        >
          Cancel
        </Button>
        <Button
          type='submit'
          size='sm'
          className='h-7 text-xs'
          disabled={register_.isPending}
          data-testid='admin-app-publisher-submit'
        >
          {register_.isPending ? "Registering…" : "Register"}
        </Button>
      </div>
    </form>
  );
};

const Field = ({
  id,
  label,
  error,
  children
}: {
  id: string;
  label: string;
  error?: string;
  children: React.ReactNode;
}) => (
  <div className='flex flex-col gap-1 text-xs'>
    <Label htmlFor={id} className='text-muted-foreground text-xs'>
      {label}
    </Label>
    {children}
    {error && <FieldError>{error}</FieldError>}
  </div>
);
