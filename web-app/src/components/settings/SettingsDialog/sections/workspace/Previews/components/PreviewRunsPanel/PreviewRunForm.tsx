import { Play } from "lucide-react";
import { useState } from "react";
import { useForm } from "react-hook-form";
import { Button } from "@/components/ui/shadcn/button";
import { Checkbox } from "@/components/ui/shadcn/checkbox";
import { Input } from "@/components/ui/shadcn/input";
import { Label } from "@/components/ui/shadcn/label";
import { Textarea } from "@/components/ui/shadcn/textarea";
import usePreviewAutomationPaths from "@/hooks/api/workspaces/usePreviewAutomationPaths";
import { useStartPreviewRun } from "@/hooks/api/workspaces/usePreviews";
import { startRunErrorMessage } from "@/libs/utils/preview";

interface FormValues {
  ref: string;
  variables: string;
}

/** Parsed variables, or a field error when the JSON is invalid. Empty input means "no variables". */
function parseVariables(
  raw: string
): { ok: true; value: Record<string, unknown> | undefined } | { ok: false; message: string } {
  if (!raw.trim()) return { ok: true, value: undefined };
  try {
    const parsed = JSON.parse(raw);
    if (parsed === null || typeof parsed !== "object" || Array.isArray(parsed)) {
      return {
        ok: false,
        message: 'Variables must be a JSON object, e.g. {"date": "2026-09-27"}.'
      };
    }
    return { ok: true, value: parsed as Record<string, unknown> };
  } catch {
    return { ok: false, message: "Variables must be valid JSON." };
  }
}

/**
 * "Dry-run a procedure": a path on the branch plus optional variables. Every
 * write the run would make is held and reported, never actually performed
 * (see the Runs list below for outcomes).
 */
export default function PreviewRunForm({
  workspaceId,
  branch
}: {
  workspaceId: string;
  branch: string;
}) {
  const start = useStartPreviewRun(workspaceId);
  const suggestions = usePreviewAutomationPaths(workspaceId, branch);
  const datalistId = `preview-run-ref-suggestions-${branch}`;
  // Not RHF-registered: `Checkbox` is a Radix button, not a native input, so
  // it takes `onCheckedChange` rather than `register`'s change event — same
  // pattern as the rest of the codebase's RHF forms with a Checkbox in them.
  const [readLiveOnly, setReadLiveOnly] = useState(false);
  const {
    register,
    handleSubmit,
    reset,
    setError,
    formState: { errors }
  } = useForm<FormValues>({ defaultValues: { ref: "", variables: "" } });

  const onSubmit = ({ ref, variables }: FormValues) => {
    const parsed = parseVariables(variables);
    if (!parsed.ok) {
      setError("variables", { type: "value", message: parsed.message });
      return;
    }
    start.mutate(
      {
        branch,
        kind: "procedure",
        ref: ref.trim(),
        variables: parsed.value,
        // Omit rather than send an explicit `false` — the server's default.
        read_live_only: readLiveOnly || undefined
      },
      {
        onSuccess: () => {
          reset();
          setReadLiveOnly(false);
        },
        onError: (err) => {
          const message = startRunErrorMessage(err);
          if (message) setError("ref", { type: "server", message });
        }
      }
    );
  };

  return (
    <form
      onSubmit={handleSubmit(onSubmit)}
      className='flex flex-col gap-2'
      data-testid='preview-run-form'
      noValidate
    >
      <Label htmlFor='preview-run-ref'>Dry-run a procedure</Label>
      <div className='flex flex-col gap-2 sm:flex-row'>
        <Input
          id='preview-run-ref'
          placeholder='workflows/compute_toast_journal_entry.procedure.yml'
          autoComplete='off'
          spellCheck={false}
          list={datalistId}
          className='font-mono sm:flex-1'
          aria-invalid={!!errors.ref}
          data-testid='preview-run-ref'
          {...register("ref", { required: "Enter a procedure path." })}
        />
        <datalist id={datalistId}>
          {suggestions.map((path) => (
            <option key={path} value={path} />
          ))}
        </datalist>
        <Button type='submit' size='sm' disabled={start.isPending} data-testid='preview-run-submit'>
          <Play />
          {start.isPending ? "Starting…" : "Dry-run"}
        </Button>
      </div>
      {errors.ref && (
        <p role='alert' className='text-destructive text-xs' data-testid='preview-run-ref-error'>
          {errors.ref.message}
        </p>
      )}
      <div className='flex items-center gap-2'>
        <Checkbox
          id='preview-run-read-live-only'
          checked={readLiveOnly}
          onCheckedChange={(checked) => setReadLiveOnly(checked === true)}
          data-testid='preview-run-read-live-only'
        />
        <Label htmlFor='preview-run-read-live-only' className='font-normal text-xs'>
          Read live tables only (writes still land in the preview)
        </Label>
      </div>
      <Textarea
        placeholder='Variables (JSON, optional) — {"date": "2026-09-27"}'
        className='font-mono text-xs'
        rows={2}
        aria-invalid={!!errors.variables}
        data-testid='preview-run-variables'
        {...register("variables")}
      />
      {errors.variables && (
        <p
          role='alert'
          className='text-destructive text-xs'
          data-testid='preview-run-variables-error'
        >
          {errors.variables.message}
        </p>
      )}
    </form>
  );
}
