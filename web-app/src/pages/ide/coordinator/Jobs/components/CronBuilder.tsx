import { CalendarClock } from "lucide-react";
import type React from "react";
import { memo, useEffect, useMemo, useState } from "react";
import { DESIGN_REFERENCE } from "@/components/ui/reference/tokens";
import { Input } from "@/components/ui/shadcn/input";
import { Label } from "@/components/ui/shadcn/label";
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue
} from "@/components/ui/shadcn/select";
import { cronNextRuns, describeCron } from "../../components/utils";

/**
 * Visual cron builder. Emits a standard 5-field cron string via `onChange`.
 * Leads with frequency presets, drops to raw cron for power users, and
 * always shows a human summary plus the next few fire times in the job's
 * timezone — which catches the day-of-month/day-of-week and TZ traps.
 */

type Mode = "minutes" | "hourly" | "daily" | "weekly" | "monthly" | "advanced";

interface Props {
  value: string;
  onChange: (cron: string) => void;
  /** IANA timezone the cron is evaluated in — drives the next-runs preview. */
  timezone: string;
}

const DAYS = [
  { v: "1", l: "Monday" },
  { v: "2", l: "Tuesday" },
  { v: "3", l: "Wednesday" },
  { v: "4", l: "Thursday" },
  { v: "5", l: "Friday" },
  { v: "6", l: "Saturday" },
  { v: "0", l: "Sunday" }
];

const CronBuilder: React.FC<Props> = ({ value, onChange, timezone }) => {
  const [mode, setMode] = useState<Mode>("daily");
  const [minute, setMinute] = useState("0");
  const [hour, setHour] = useState("9");
  const [everyN, setEveryN] = useState("15");
  const [dow, setDow] = useState("1");
  const [dom, setDom] = useState("1");
  const [advanced, setAdvanced] = useState(value || "0 9 * * *");

  // Recompute the cron string whenever the relevant inputs change.
  useEffect(() => {
    let cron: string;
    switch (mode) {
      case "minutes":
        cron = `*/${everyN || "1"} * * * *`;
        break;
      case "hourly":
        cron = `${minute || "0"} * * * *`;
        break;
      case "daily":
        cron = `${minute || "0"} ${hour || "0"} * * *`;
        break;
      case "weekly":
        cron = `${minute || "0"} ${hour || "0"} * * ${dow}`;
        break;
      case "monthly":
        cron = `${minute || "0"} ${hour || "0"} ${dom || "1"} * *`;
        break;
      default:
        cron = advanced.trim();
    }
    onChange(cron);
    // onChange identity is stable enough for this controlled emitter.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [mode, minute, hour, everyN, dow, dom, advanced, onChange]);

  const nextRuns = useMemo(() => cronNextRuns(value, timezone, 3), [value, timezone]);

  return (
    <div className='flex flex-col gap-3'>
      <div className='flex flex-col gap-2'>
        <Label>Frequency</Label>
        <Select value={mode} onValueChange={(m) => setMode(m as Mode)}>
          <SelectTrigger>
            <SelectValue />
          </SelectTrigger>
          <SelectContent className={DESIGN_REFERENCE}>
            <SelectItem value='minutes'>Every N minutes</SelectItem>
            <SelectItem value='hourly'>Hourly</SelectItem>
            <SelectItem value='daily'>Daily</SelectItem>
            <SelectItem value='weekly'>Weekly</SelectItem>
            <SelectItem value='monthly'>Monthly</SelectItem>
            <SelectItem value='advanced'>Advanced (raw cron)</SelectItem>
          </SelectContent>
        </Select>
      </div>

      {mode === "minutes" && (
        <div className='flex flex-col gap-2'>
          <Label htmlFor='everyN'>Every</Label>
          <div className='flex items-center gap-2'>
            <Input
              id='everyN'
              type='number'
              min={1}
              max={59}
              className='w-20'
              value={everyN}
              onChange={(e) => setEveryN(e.target.value)}
            />
            <span className='text-muted-foreground text-sm'>minute(s)</span>
          </div>
        </div>
      )}

      {mode === "hourly" && (
        <div className='flex flex-col gap-2'>
          <Label htmlFor='minute'>At minute</Label>
          <Input
            id='minute'
            type='number'
            min={0}
            max={59}
            className='w-20'
            value={minute}
            onChange={(e) => setMinute(e.target.value)}
          />
        </div>
      )}

      {(mode === "daily" || mode === "weekly" || mode === "monthly") && (
        <div className='flex flex-wrap items-end gap-3'>
          {mode === "weekly" && (
            <div className='flex flex-col gap-2'>
              <Label>On</Label>
              <Select value={dow} onValueChange={setDow}>
                <SelectTrigger className='w-36'>
                  <SelectValue />
                </SelectTrigger>
                <SelectContent className={DESIGN_REFERENCE}>
                  {DAYS.map((d) => (
                    <SelectItem key={d.v} value={d.v}>
                      {d.l}
                    </SelectItem>
                  ))}
                </SelectContent>
              </Select>
            </div>
          )}
          {mode === "monthly" && (
            <div className='flex flex-col gap-2'>
              <Label htmlFor='dom'>Day of month</Label>
              <Input
                id='dom'
                type='number'
                min={1}
                max={31}
                className='w-20'
                value={dom}
                onChange={(e) => setDom(e.target.value)}
              />
            </div>
          )}
          <div className='flex flex-col gap-2'>
            <Label htmlFor='at'>At time</Label>
            <div className='flex items-center gap-2'>
              <Input
                id='at'
                type='number'
                min={0}
                max={23}
                className='w-20'
                value={hour}
                onChange={(e) => setHour(e.target.value)}
              />
              <span className='text-muted-foreground text-sm'>:</span>
              <Input
                type='number'
                min={0}
                max={59}
                className='w-20'
                value={minute}
                onChange={(e) => setMinute(e.target.value)}
              />
            </div>
          </div>
        </div>
      )}

      {mode === "advanced" && (
        <div className='flex flex-col gap-2'>
          <Label htmlFor='rawcron'>Cron expression</Label>
          <Input
            id='rawcron'
            placeholder='0 9 * * *'
            className='font-mono'
            value={advanced}
            onChange={(e) => setAdvanced(e.target.value)}
          />
          <p className='text-muted-foreground text-xs'>
            Standard 5-field cron (min hour day-of-month month day-of-week). Validated on save.
          </p>
        </div>
      )}

      <div className='flex flex-col gap-1.5 bg-muted px-3 py-2'>
        <div className='flex items-center gap-2'>
          <code className='font-mono text-sm'>{value || "—"}</code>
          <span className='text-muted-foreground text-xs'>{describeCron(value)}</span>
        </div>
        <div className='flex items-start gap-1.5 text-muted-foreground text-xs'>
          <CalendarClock className='mt-0.5 h-3.5 w-3.5 shrink-0' />
          <span>
            Next in {timezone}:{" "}
            {nextRuns.length === 0
              ? "no upcoming runs"
              : nextRuns
                  .map((d) =>
                    d.toLocaleString(undefined, {
                      month: "short",
                      day: "numeric",
                      hour: "2-digit",
                      minute: "2-digit"
                    })
                  )
                  .join("  ·  ")}
          </span>
        </div>
      </div>
    </div>
  );
};

// Memoized so unrelated dialog state changes (name, question, etc.) don't
// re-render this whole subtree on every keystroke. Props are primitives,
// so the default shallow comparison is sufficient.
export default memo(CronBuilder);
