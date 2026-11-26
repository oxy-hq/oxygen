import type React from "react";
import { useEffect, useMemo, useState } from "react";
import { toast } from "sonner";
import { DESIGN_REFERENCE } from "@/components/ui/reference/tokens";
import { Button } from "@/components/ui/shadcn/button";
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogTitle
} from "@/components/ui/shadcn/dialog";
import { Input } from "@/components/ui/shadcn/input";
import { Label } from "@/components/ui/shadcn/label";
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue
} from "@/components/ui/shadcn/select";
import { Switch } from "@/components/ui/shadcn/switch";
import { Textarea } from "@/components/ui/shadcn/textarea";
import { useAgenticAutomationFiles } from "@/hooks/api/agentic-automations/useAgenticAutomations";
import {
  useAirwayFiles,
  useCreateSchedule,
  useScheduleAgents,
  useUpdateSchedule
} from "@/hooks/api/schedules/useSchedules";
import type { Schedule, ScheduleInput, ScheduleTargetKind } from "@/types/schedule";
import CronBuilder from "./CronBuilder";

const TIMEZONES: string[] = (() => {
  if ("supportedValuesOf" in Intl) {
    return (Intl as unknown as { supportedValuesOf: (k: string) => string[] }).supportedValuesOf(
      "timeZone"
    );
  }
  return ["UTC"];
})();

// The IANA list is 400+ entries — building the JSX inside render means
// 400+ React.createElement calls per keystroke (since every controlled-
// input change re-renders this whole dialog). Allocate the items once
// at module scope so per-render work is O(1).
const TIMEZONE_ITEMS = TIMEZONES.map((tz) => (
  <SelectItem key={tz} value={tz}>
    {tz}
  </SelectItem>
));

const FREE_TEXT = "__free_text__";

/**
 * Parse the variables textarea into what `ScheduleInput.variables` wants.
 *
 * Empty → `null` (no overrides). Anything else must be a JSON OBJECT: the
 * backend folds it over the target's declared `variables:` defaults key by
 * key, so an array or a scalar has nothing to fold and would be silently
 * ignored rather than rejected.
 */
function parseVariables(
  text: string
): { ok: true; value: Record<string, unknown> | null } | { ok: false; error: string } {
  const trimmed = text.trim();
  if (!trimmed) return { ok: true, value: null };
  let parsed: unknown;
  try {
    parsed = JSON.parse(trimmed);
  } catch (e) {
    return { ok: false, error: e instanceof Error ? e.message : "Invalid JSON" };
  }
  if (parsed === null || typeof parsed !== "object" || Array.isArray(parsed)) {
    return { ok: false, error: 'Must be a JSON object, e.g. {"lookback_days": 3}' };
  }
  return { ok: true, value: parsed as Record<string, unknown> };
}

interface Props {
  open: boolean;
  onOpenChange: (open: boolean) => void;
  /** Present = edit; absent = create. */
  schedule?: Schedule | null;
}

/** Create / edit form for a scheduled job (automation or airway pipeline). */
const ScheduleDialog: React.FC<Props> = ({ open, onOpenChange, schedule }) => {
  const isEdit = !!schedule;
  const createMut = useCreateSchedule();
  const updateMut = useUpdateSchedule();

  const [name, setName] = useState("");
  const [targetKind, setTargetKind] = useState<ScheduleTargetKind>("workflow");
  const [targetRef, setTargetRef] = useState("");
  const [question, setQuestion] = useState("");
  const [cron, setCron] = useState("0 9 * * *");
  const [timezone, setTimezone] = useState("UTC");
  const [enabled, setEnabled] = useState(true);
  // Held as TEXT, not a parsed object, so a half-typed edit survives a
  // re-render instead of being reformatted under the cursor.
  const [variablesText, setVariablesText] = useState("");
  const [variablesError, setVariablesError] = useState<string | null>(null);

  // Reset/prefill whenever the dialog opens.
  // biome-ignore lint/correctness/useExhaustiveDependencies: keyed on schedule.id so a refetch mid-edit cannot clobber the form — see the dep array
  useEffect(() => {
    if (!open) return;
    setName(schedule?.name ?? "");
    setTargetKind(schedule?.target_kind ?? "workflow");
    setTargetRef(schedule?.target_ref ?? "");
    setQuestion(schedule?.question ?? "");
    setCron(schedule?.cron_expr ?? "0 9 * * *");
    setTimezone(schedule?.timezone ?? "UTC");
    setEnabled(schedule?.enabled ?? true);
    setVariablesText(schedule?.variables ? JSON.stringify(schedule.variables, null, 2) : "");
    setVariablesError(null);
    // Keyed on the id, not the object: `next_run_at` / `last_fired_at` /
    // `missed_runs` advance on every fire, so depending on the row itself
    // means a job firing while the dialog is open re-runs this prefill and
    // overwrites whatever is half-typed — most painfully the JSON body.
  }, [open, schedule?.id]);

  const { data: automationFiles } = useAgenticAutomationFiles();
  const { data: airwayFiles } = useAirwayFiles();
  const { data: agentFiles } = useScheduleAgents();
  const refs = useMemo(() => {
    if (targetKind === "workflow") return automationFiles?.map((f) => f.path) ?? [];
    if (targetKind === "airway") return airwayFiles?.map((f) => f.path) ?? [];
    return agentFiles?.map((a) => a.path) ?? [];
  }, [targetKind, automationFiles, airwayFiles, agentFiles]);
  // The picker selects a known ref or "free text"; in free-text mode the
  // ref comes from the input below.
  const isKnownRef = refs.includes(targetRef);
  const [freeText, setFreeText] = useState(false);
  useEffect(() => {
    if (open) setFreeText(!!targetRef && !isKnownRef);
    // Only re-evaluate when the dialog (re)opens.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [open, targetRef, isKnownRef]);

  const submit = async () => {
    if (!name.trim()) {
      toast.error("Name is required");
      return;
    }
    if (!targetRef.trim()) {
      toast.error("Target is required");
      return;
    }
    if (targetKind === "agent" && !question.trim()) {
      toast.error("Question is required for agent schedules");
      return;
    }
    const vars = parseVariables(variablesText);
    if (!vars.ok) {
      setVariablesError(vars.error);
      toast.error(`Variables: ${vars.error}`);
      return;
    }
    const input: ScheduleInput = {
      name: name.trim(),
      target_kind: targetKind,
      target_ref: targetRef.trim(),
      question: targetKind === "agent" ? question.trim() : null,
      // Sent for EVERY target kind, including the ones with no editor below.
      // The backend PATCH is a whole-row write (`variables: Set(input.variables)`),
      // so omitting this clears whatever the schedule already had — which for a
      // monitor_scan means dropping `granularity` and leaving the scan to fail
      // every fire with "missing granularity in variables". Round-tripping the
      // prefilled text keeps an edit from destroying state the form can't show.
      variables: vars.value,
      cron_expr: cron.trim(),
      timezone,
      enabled
    };
    try {
      if (isEdit && schedule) {
        await updateMut.mutateAsync({ id: schedule.id, input });
      } else {
        await createMut.mutateAsync(input);
      }
      onOpenChange(false);
    } catch {
      // Surfaced by the mutation's onError toast (incl. backend 400s).
    }
  };

  return (
    <Dialog open={open} onOpenChange={onOpenChange}>
      <DialogContent className={`${DESIGN_REFERENCE} max-h-[85vh] overflow-y-auto sm:max-w-lg`}>
        <DialogHeader>
          <DialogTitle>{isEdit ? "Edit job" : "Create job"}</DialogTitle>
          <DialogDescription>
            Run a DAG automation, ELT pipeline, agent, or metric monitor scan on a recurring cron
            schedule.
          </DialogDescription>
        </DialogHeader>

        <div className='flex flex-col gap-4'>
          <div className='flex flex-col gap-2'>
            <Label htmlFor='sched-name'>Name</Label>
            <Input
              id='sched-name'
              placeholder='e.g., Daily revenue report'
              value={name}
              onChange={(e) => setName(e.target.value)}
            />
          </div>

          <div className='flex flex-wrap items-end gap-3'>
            <div className='flex flex-col gap-2'>
              <Label>Job type</Label>
              <Select
                value={targetKind}
                onValueChange={(v) => {
                  setTargetKind(v as ScheduleTargetKind);
                  setTargetRef("");
                  setFreeText(false);
                  // Variables are target-specific, and the editor only renders
                  // for workflow/airway while parseVariables runs on EVERY
                  // submit. Left behind, a half-typed body blocks the save
                  // with a toast about a field that is no longer on screen.
                  setVariablesText("");
                  setVariablesError(null);
                }}
                disabled={targetKind === "monitor_scan"}
              >
                <SelectTrigger className='w-44'>
                  <SelectValue />
                </SelectTrigger>
                <SelectContent className={DESIGN_REFERENCE}>
                  <SelectItem value='workflow'>DAG automation</SelectItem>
                  <SelectItem value='airway'>ELT pipeline</SelectItem>
                  <SelectItem value='agent'>Agent</SelectItem>
                  {targetKind === "monitor_scan" && (
                    <SelectItem value='monitor_scan'>Monitor scan</SelectItem>
                  )}
                </SelectContent>
              </Select>
            </div>

            {targetKind !== "monitor_scan" && (
              <div className='flex min-w-60 flex-1 flex-col gap-2'>
                <Label>{targetKind === "agent" ? "Agent" : "Target file"}</Label>
                {freeText ? (
                  <Input
                    placeholder='workspace-relative path'
                    className='font-mono'
                    value={targetRef}
                    onChange={(e) => setTargetRef(e.target.value)}
                  />
                ) : (
                  <Select
                    value={isKnownRef ? targetRef : ""}
                    onValueChange={(v) => {
                      if (v === FREE_TEXT) {
                        setFreeText(true);
                        setTargetRef("");
                      } else {
                        setTargetRef(v);
                      }
                    }}
                  >
                    <SelectTrigger>
                      <SelectValue
                        placeholder={targetKind === "agent" ? "Select an agent" : "Select a file"}
                      />
                    </SelectTrigger>
                    <SelectContent className={DESIGN_REFERENCE}>
                      {refs.map((r) => (
                        <SelectItem key={r} value={r}>
                          {r}
                        </SelectItem>
                      ))}
                      <SelectItem value={FREE_TEXT}>Other (type a path)…</SelectItem>
                    </SelectContent>
                  </Select>
                )}
              </div>
            )}

            {targetKind === "monitor_scan" && (
              <div className='flex min-w-60 flex-1 flex-col gap-2'>
                <Label>Granularity</Label>
                <div className='flex h-9 items-center border border-input bg-muted px-3 text-muted-foreground text-sm'>
                  {(schedule?.variables as Record<string, string> | null)?.granularity ?? "—"}
                </div>
              </div>
            )}
          </div>

          {targetKind === "agent" && (
            <div className='flex flex-col gap-2'>
              <Label htmlFor='sched-question'>Question</Label>
              <Textarea
                id='sched-question'
                placeholder='What should the agent answer on every run?'
                rows={3}
                value={question}
                onChange={(e) => setQuestion(e.target.value)}
              />
            </div>
          )}

          {(targetKind === "workflow" || targetKind === "airway") && (
            <div className='flex flex-col gap-2'>
              <Label htmlFor='sched-variables'>Variables</Label>
              <Textarea
                id='sched-variables'
                className='font-mono text-xs'
                rows={4}
                placeholder={'{\n  "lookback_days": 3\n}'}
                value={variablesText}
                onChange={(e) => {
                  setVariablesText(e.target.value);
                  setVariablesError(null);
                }}
                aria-invalid={!!variablesError}
                aria-describedby='sched-variables-help'
              />
              {variablesError ? (
                // Same id as the help text: `aria-describedby` is always set,
                // so the id has to resolve in this branch too or a reader
                // announces "invalid" and nothing else. `alert` makes the
                // message speak when it appears.
                <p id='sched-variables-help' role='alert' className='text-destructive text-xs'>
                  {variablesError}
                </p>
              ) : (
                <p id='sched-variables-help' className='text-muted-foreground text-xs'>
                  Optional JSON object, folded over the{" "}
                  {targetKind === "airway" ? "pipeline" : "automation"}&rsquo;s declared defaults on
                  every run. These values are <strong>static</strong> &mdash; the same ones are sent
                  on every fire, so a moving window needs a relative variable the target resolves
                  itself (say <code>lookback_days</code>), not fixed dates that would replay one
                  frozen range forever.
                </p>
              )}
            </div>
          )}

          <CronBuilder value={cron} onChange={setCron} timezone={timezone} />

          <div className='flex flex-col gap-2'>
            <Label>Timezone</Label>
            <Select value={timezone} onValueChange={setTimezone}>
              <SelectTrigger>
                <SelectValue />
              </SelectTrigger>
              <SelectContent className={`${DESIGN_REFERENCE} max-h-72`}>
                {TIMEZONE_ITEMS}
              </SelectContent>
            </Select>
          </div>

          <div className='flex items-center justify-between'>
            <Label htmlFor='sched-enabled'>Enabled</Label>
            <Switch id='sched-enabled' checked={enabled} onCheckedChange={setEnabled} />
          </div>
        </div>

        <DialogFooter>
          <Button variant='outline' onClick={() => onOpenChange(false)}>
            Cancel
          </Button>
          <Button
            data-testid='coordinator-schedule-save'
            onClick={submit}
            disabled={createMut.isPending || updateMut.isPending}
          >
            {isEdit ? "Save changes" : "Create job"}
          </Button>
        </DialogFooter>
      </DialogContent>
    </Dialog>
  );
};

export default ScheduleDialog;
