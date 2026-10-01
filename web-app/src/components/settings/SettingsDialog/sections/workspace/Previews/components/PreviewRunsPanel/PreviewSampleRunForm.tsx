import { Beaker } from "lucide-react";
import { useForm } from "react-hook-form";
import { Button } from "@/components/ui/shadcn/button";
import { Input } from "@/components/ui/shadcn/input";
import { Label } from "@/components/ui/shadcn/label";
import { useStartPreviewRun } from "@/hooks/api/workspaces/usePreviews";
import { sampleRunErrorMessage } from "@/libs/utils/preview";
import type { PreviewRunWindow } from "@/types/workspace";

interface FormValues {
  ref: string;
  from: string;
  to: string;
  resources: string;
}

const toRfc3339Start = (date: string): string => `${date}T00:00:00.000Z`;

/** Exclusive upper bound: the calendar day after `date`, at UTC midnight — same convention as the Airway backfill modal's own "end date (inclusive)" field. */
function toRfc3339EndExclusive(date: string): string {
  const next = new Date(`${date}T00:00:00.000Z`);
  next.setUTCDate(next.getUTCDate() + 1);
  return next.toISOString();
}

/** `from`/`to` both set, both blank (server default: last 7 days), or a field error. */
function parseWindow(
  from: string,
  to: string
): { ok: true; value: PreviewRunWindow | undefined } | { ok: false; message: string } {
  if (!from && !to) return { ok: true, value: undefined };
  if (!from || !to) {
    return { ok: false, message: "Enter both a start and an end, or leave both blank." };
  }
  return { ok: true, value: { from: toRfc3339Start(from), to: toRfc3339EndExclusive(to) } };
}

/** Comma-separated resource names, trimmed and emptied out — `undefined` when nothing was typed. */
function parseResources(raw: string): string[] | undefined {
  const items = raw
    .split(",")
    .map((r) => r.trim())
    .filter(Boolean);
  return items.length > 0 ? items : undefined;
}

/**
 * "Sample an Airway pipeline": a bounded window of a branch's pipeline into
 * the preview, held next to the procedure dry-run form above. A rotate-on-use
 * source (QuickBooks) needs a sandbox source registered first — see the
 * Sandbox sources panel — or this refuses with `sandbox_required`.
 */
export default function PreviewSampleRunForm({
  workspaceId,
  branch
}: {
  workspaceId: string;
  branch: string;
}) {
  const start = useStartPreviewRun(workspaceId);
  const {
    register,
    handleSubmit,
    reset,
    setError,
    formState: { errors }
  } = useForm<FormValues>({ defaultValues: { ref: "", from: "", to: "", resources: "" } });

  const onSubmit = ({ ref, from, to, resources }: FormValues) => {
    const window = parseWindow(from, to);
    if (!window.ok) {
      setError("to", { type: "value", message: window.message });
      return;
    }
    start.mutate(
      {
        branch,
        kind: "airway_sample",
        ref: ref.trim(),
        window: window.value,
        resources: parseResources(resources)
      },
      {
        onSuccess: () => reset(),
        onError: (err) => {
          const message = sampleRunErrorMessage(err);
          if (message) setError("ref", { type: "server", message });
        }
      }
    );
  };

  return (
    <form
      onSubmit={handleSubmit(onSubmit)}
      className='flex flex-col gap-2'
      data-testid='preview-sample-run-form'
      noValidate
    >
      <Label htmlFor='preview-sample-ref'>Sample an Airway pipeline</Label>
      <div className='flex flex-col gap-2 sm:flex-row'>
        <Input
          id='preview-sample-ref'
          placeholder='pipelines/quickbooks_financials_eastbay.airway.yml'
          autoComplete='off'
          spellCheck={false}
          className='font-mono sm:flex-1'
          aria-invalid={!!errors.ref}
          data-testid='preview-sample-ref'
          {...register("ref", { required: "Enter a pipeline path." })}
        />
        <Button
          type='submit'
          size='sm'
          disabled={start.isPending}
          data-testid='preview-sample-submit'
        >
          <Beaker />
          {start.isPending ? "Starting…" : "Sample"}
        </Button>
      </div>
      {errors.ref && (
        <p role='alert' className='text-destructive text-xs' data-testid='preview-sample-ref-error'>
          {errors.ref.message}
        </p>
      )}
      <div className='flex flex-col gap-2 sm:flex-row sm:items-end'>
        <div className='flex flex-col gap-1'>
          <Label htmlFor='preview-sample-from' className='font-normal text-xs'>
            From
          </Label>
          <Input
            id='preview-sample-from'
            type='date'
            data-testid='preview-sample-from'
            {...register("from")}
          />
        </div>
        <div className='flex flex-col gap-1'>
          <Label htmlFor='preview-sample-to' className='font-normal text-xs'>
            To
          </Label>
          <Input
            id='preview-sample-to'
            type='date'
            data-testid='preview-sample-to'
            {...register("to")}
          />
        </div>
        <Input
          placeholder='Resources (comma-separated, optional)'
          className='font-mono text-xs sm:flex-1'
          data-testid='preview-sample-resources'
          {...register("resources")}
        />
      </div>
      {errors.to && (
        <p role='alert' className='text-destructive text-xs' data-testid='preview-sample-to-error'>
          {errors.to.message}
        </p>
      )}
      <p className='text-muted-foreground text-xs'>
        Leave the window blank for the last 7 days — windowed sources only (Toast, QuickBooks); max
        31 days.
      </p>
    </form>
  );
}
