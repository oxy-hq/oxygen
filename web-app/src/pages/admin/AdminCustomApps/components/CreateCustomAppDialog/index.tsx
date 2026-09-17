import { AxiosError } from "axios";
import { AlertTriangle, ExternalLink } from "lucide-react";
import { useState } from "react";
import { Controller, useForm } from "react-hook-form";
import { Button } from "@/components/ui/shadcn/button";
import { Combobox } from "@/components/ui/shadcn/combobox";
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogTitle
} from "@/components/ui/shadcn/dialog";
import { FieldError } from "@/components/ui/shadcn/field";
import { Input } from "@/components/ui/shadcn/input";
import { Label } from "@/components/ui/shadcn/label";
import { useCreateApp } from "@/hooks/api/customApps/useCreateApp";
import { useOrgs } from "@/hooks/api/organizations";
import { useAllWorkspaces } from "@/hooks/api/workspaces/useWorkspaces";
import type { CustomApp } from "@/types/apps";
import { resolveBundleUrl } from "../../resolveBundleUrl";

type Props = {
  open: boolean;
  onOpenChange: (open: boolean) => void;
};

type FormValues = {
  name: string;
  org_id: string;
  project_id: string;
  branch: string;
};

/**
 * Register a custom app. That is all it does: the row is created in the build
 * store's name, and bytes arrive afterwards with `oxyc publish`.
 *
 * There used to be three intents here — link a folder on the oxy host, create
 * (which, in local mode, provisioned such a folder), and wrap a Vercel URL.
 * The folder and URL sources were removed on 2026-09-17, which left one path,
 * so the dialog is a plain form.
 */
export const CreateCustomAppDialog = ({ open, onOpenChange }: Props) => {
  const { mutateAsync, isPending } = useCreateApp();
  const { data: orgs, isLoading: orgsLoading } = useOrgs();
  const [result, setResult] = useState<CustomApp | null>(null);
  // Inline submit error surfaced from the server's structured response
  // body (`{ message }`). Distinct from toast errors because slug
  // conflicts and other 4xx cases are blocking — the operator can't
  // proceed until they fix it, so the message has to stay visible.
  const [submitError, setSubmitError] = useState<string | null>(null);

  const {
    register,
    handleSubmit,
    reset,
    control,
    watch,
    formState: { errors }
  } = useForm<FormValues>({ defaultValues: { branch: "main" } });

  const selectedOrgId = watch("org_id");

  const onSubmit = async (data: FormValues) => {
    setSubmitError(null);
    try {
      const created = await mutateAsync({
        name: data.name,
        org_id: data.org_id,
        project_id: data.project_id,
        branch: data.branch || undefined,
        source: { type: "s3" }
      });
      setResult(created);
      reset();
    } catch (err) {
      setSubmitError(extractServerMessage(err) ?? "Failed to create app.");
    }
  };

  const close = (v: boolean) => {
    onOpenChange(v);
    if (!v) {
      setResult(null);
      setSubmitError(null);
    }
  };

  return (
    <Dialog open={open} onOpenChange={close}>
      {/* max-h-[85vh] + flex column keeps the dialog inside the viewport; the
          form is the scroll container so header + footer stay anchored.
          overflow-hidden + min-w-0 clamp long URLs horizontally. */}
      <DialogContent
        className='flex max-h-[85vh] max-w-xl flex-col overflow-hidden'
        data-testid='admin-apps-create-dialog'
      >
        <DialogHeader>
          <DialogTitle>{result ? "App created" : "Add custom app"}</DialogTitle>
          {!result && (
            <DialogDescription className='text-xs'>
              Registers the app. Nothing is uploaded here — you ship builds to it with{" "}
              <code className='whitespace-nowrap font-mono'>oxyc publish</code>, and the commands
              appear once it exists.
            </DialogDescription>
          )}
        </DialogHeader>

        {!result && (
          <form
            onSubmit={handleSubmit(onSubmit)}
            className='flex min-h-0 min-w-0 flex-1 flex-col gap-4'
          >
            {/* Body scrolls so the footer stays anchored at the bottom.
                pr-1 prevents the scrollbar from overlapping focus rings. */}
            <div className='flex min-h-0 flex-1 flex-col gap-4 overflow-y-auto pr-1'>
              <div className='flex flex-col gap-1.5'>
                <Label htmlFor='app-name'>Name</Label>
                <Input
                  id='app-name'
                  placeholder='Store Pulse'
                  {...register("name", { required: "Required" })}
                />
                {errors.name && <FieldError>{errors.name.message}</FieldError>}
              </div>

              <OrgPicker
                control={control}
                orgs={orgs}
                isLoading={orgsLoading}
                error={errors.org_id}
              />

              <ProjectPicker control={control} orgId={selectedOrgId} error={errors.project_id} />

              {submitError && (
                <div
                  role='alert'
                  className='flex items-start gap-2 rounded-md border border-destructive/40 bg-destructive/10 p-3 text-destructive text-xs'
                >
                  <AlertTriangle className='mt-0.5 size-4 shrink-0' />
                  <span className='min-w-0 flex-1 break-words'>{submitError}</span>
                </div>
              )}
            </div>
            <DialogFooter>
              <Button type='submit' disabled={isPending} data-testid='admin-apps-create-submit'>
                {isPending ? "Creating…" : "Create"}
              </Button>
            </DialogFooter>
          </form>
        )}

        {result && <CreatedSummary app={result} onDone={() => close(false)} />}
      </DialogContent>
    </Dialog>
  );
};

/** RFC 4122 shape check for the manual-UUID escape hatches below. */
const isUuidShape = (s: string | undefined): boolean =>
  !!s && /^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$/i.test(s.trim());

/** Pull the human message out of an axios error response. The server
 *  returns `{ message: "..." }` for 4xx/5xx on the admin apps surface
 *  (see backend's `ErrorBody`); falls back to the JS error message
 *  for unexpected throws. Returns null if nothing useful is available
 *  so the caller can default to a generic copy. */
const extractServerMessage = (err: unknown): string | null => {
  if (err instanceof AxiosError) {
    const data = err.response?.data;
    if (data && typeof data === "object" && "message" in data) {
      const msg = (data as { message?: unknown }).message;
      if (typeof msg === "string" && msg.trim()) return msg;
    }
  }
  if (err instanceof Error && err.message) return err.message;
  return null;
};

type OrgPickerProps = {
  control: import("react-hook-form").Control<FormValues>;
  orgs: { id: string; name: string; slug: string }[] | undefined;
  isLoading: boolean;
  error: import("react-hook-form").FieldError | undefined;
};

/**
 * Searchable org picker with a "paste a UUID instead" escape hatch.
 * Global Admins can be registering apps for orgs they're not a member
 * of — those won't surface in `useOrgs()` and the only option is
 * to paste the uuid.
 */
const OrgPicker = ({ control, orgs, isLoading, error }: OrgPickerProps) => {
  const [manual, setManual] = useState(false);
  const items = (orgs ?? []).map((org) => ({
    value: org.id,
    label: org.name,
    searchText: `${org.name} ${org.slug}`
  }));

  return (
    <div className='flex flex-col gap-1.5'>
      <Label htmlFor='app-org-id'>Organization</Label>
      <Controller
        name='org_id'
        control={control}
        rules={{
          required: "Required",
          validate: (v) => isUuidShape(v) || "Must be a valid UUID"
        }}
        render={({ field }) =>
          manual ? (
            <Input
              id='app-org-id'
              placeholder='xxxxxxxx-xxxx-xxxx-xxxx-xxxxxxxxxxxx'
              value={field.value ?? ""}
              onChange={(e) => field.onChange(e.target.value)}
            />
          ) : (
            <Combobox
              items={items}
              value={field.value || undefined}
              onValueChange={field.onChange}
              placeholder={isLoading ? "Loading orgs…" : "Select an organization"}
              searchPlaceholder='Search by name or slug…'
              disabled={isLoading}
              renderItem={(item) => {
                const org = orgs?.find((o) => o.id === item.value);
                return (
                  <span className='flex flex-1 items-center justify-between'>
                    <span>{item.label}</span>
                    {org && <span className='text-muted-foreground text-xs'>{org.slug}</span>}
                  </span>
                );
              }}
            />
          )
        }
      />
      <button
        type='button'
        onClick={() => setManual(!manual)}
        className='self-start text-muted-foreground text-xs hover:text-foreground'
      >
        {manual ? "Pick from the list instead" : "Paste a UUID instead"}
      </button>
      {error && <FieldError>{error.message}</FieldError>}
    </div>
  );
};

type ProjectPickerProps = {
  control: import("react-hook-form").Control<FormValues>;
  orgId: string | undefined;
  error: import("react-hook-form").FieldError | undefined;
};

const ProjectPicker = ({ control, orgId, error }: ProjectPickerProps) => {
  const { data: workspaces, isLoading } = useAllWorkspaces(orgId);
  const [manual, setManual] = useState(false);
  const hasOrg = !!orgId;
  const items = (workspaces ?? []).map((ws) => ({
    value: ws.id,
    label: ws.name,
    searchText: `${ws.name} ${ws.id}`
  }));

  const placeholder = !hasOrg
    ? "Select an organization first"
    : isLoading
      ? "Loading workspaces…"
      : items.length === 0
        ? "No workspaces in this org — paste a UUID"
        : "Select a workspace";

  return (
    <div className='flex flex-col gap-1.5'>
      <Label htmlFor='app-project-id'>Project (workspace)</Label>
      <Controller
        name='project_id'
        control={control}
        rules={{
          required: "Required",
          validate: (v) => isUuidShape(v) || "Must be a valid UUID"
        }}
        render={({ field }) =>
          manual ? (
            <Input
              id='app-project-id'
              placeholder='xxxxxxxx-xxxx-xxxx-xxxx-xxxxxxxxxxxx'
              value={field.value ?? ""}
              onChange={(e) => field.onChange(e.target.value)}
            />
          ) : (
            <Combobox
              items={items}
              value={field.value || undefined}
              onValueChange={field.onChange}
              placeholder={placeholder}
              searchPlaceholder='Search by workspace name…'
              disabled={!hasOrg || isLoading}
              renderItem={(item) => (
                <span className='flex flex-1 items-center justify-between'>
                  <span>{item.label}</span>
                  <span className='text-muted-foreground text-xs'>{item.value.slice(0, 8)}</span>
                </span>
              )}
            />
          )
        }
      />
      <button
        type='button'
        onClick={() => setManual(!manual)}
        className='self-start text-muted-foreground text-xs hover:text-foreground'
      >
        {manual ? "Pick from the list instead" : "Paste a UUID instead"}
      </button>
      {error && <FieldError>{error.message}</FieldError>}
    </div>
  );
};

const CreatedSummary = ({ app, onDone }: { app: CustomApp; onDone: () => void }) => (
  // min-w-0 throughout — long PR URLs would otherwise widen the modal past
  // max-w-xl. Inline `<code>` blocks can't break on a slash, so we lean on
  // `break-all` for them.
  <div className='flex min-w-0 flex-col gap-3 text-xs'>
    <div className='flex min-w-0 flex-col gap-1'>
      <SummaryRow label='ID'>
        <code className='break-all rounded bg-muted px-1 py-0.5 font-mono text-xs'>{app.id}</code>
      </SummaryRow>
      <SummaryRow label='URL'>
        <a
          href={resolveBundleUrl(app.url)}
          className='inline-flex items-center gap-1 break-all text-primary underline underline-offset-4'
          target='_blank'
          rel='noopener noreferrer'
        >
          {app.url} <ExternalLink className='size-3' />
        </a>
      </SummaryRow>
      {app.bootstrap_pr_url && (
        <SummaryRow label='Scaffold PR'>
          <a
            href={app.bootstrap_pr_url}
            className='inline-flex items-center gap-1 break-all text-primary underline underline-offset-4'
            target='_blank'
            rel='noopener noreferrer'
          >
            {app.bootstrap_pr_url} <ExternalLink className='size-3' />
          </a>
        </SummaryRow>
      )}
    </div>

    <NextSteps app={app} />

    <DialogFooter>
      <Button onClick={onDone}>Done</Button>
    </DialogFooter>
  </div>
);

const SummaryRow = ({ label, children }: { label: string; children: React.ReactNode }) => (
  <p>
    <span className='font-medium'>{label}:</span> {children}
  </p>
);

const NextSteps = ({ app }: { app: CustomApp }) => (
  <div className='flex flex-col gap-1.5'>
    <p className='font-medium'>
      Next: ship a build with <code className='font-mono'>oxyc publish</code>
    </p>
    <p className='text-muted-foreground text-xs'>
      The app is registered, so publish resolves the project automatically — no{" "}
      <code className='font-mono'>--project</code> needed.
    </p>
    <pre className='overflow-x-auto rounded bg-muted px-2 py-1.5 font-mono text-foreground text-xs'>
      {`# from your app dir — oxy-app.json: { "slug": "${app.slug}", "orgSlug": "${app.org_slug}" }
npm install -g @oxy-hq/cli               # once
oxyc login --env production
oxyc publish --env production            # → draft
oxyc publish --env production --promote  # → live`}
    </pre>
    <p className='text-muted-foreground text-xs'>
      No app code yet? Scaffold one:{" "}
      <code className='font-mono'>pnpm dlx create-oxy-app {app.slug} --template vite</code>
    </p>
  </div>
);
