import { useEffect, useMemo, useRef, useState } from "react";
import { DESIGN_REFERENCE } from "@/components/ui/reference/tokens";
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue
} from "@/components/ui/shadcn/select";
import { usePredict } from "@/hooks/api/useMetricTree";
import type { PredictImpact, SensitivityDriver } from "@/types/metricTree";
import {
  CONFIDENCE_HELP,
  FORM_HELP,
  InfoTip,
  MetaBadge,
  Row,
  SectionHeader,
  SectionSpinner
} from "../../components/semanticGraph";
import { formatDelta, shortMeasureName } from "./measureTarget";

const DEBOUNCE_MS = 300;

interface WhatIfResult {
  impacts: PredictImpact[] | undefined;
  error: Error | null;
  isPending: boolean;
}

const IDLE_RESULT: WhatIfResult = { impacts: undefined, error: null, isPending: false };

/**
 * Size a lever rather than a gap: move one driver by a delta (in that driver's
 * own units) and propagate it up the tree to the target.
 *
 * Propagation is a pure metric-tree walk server-side (no warehouse query), so it
 * is free to re-run — the what-if updates live as you change the driver or the
 * delta rather than behind a manual "run", debounced only to coalesce keystrokes.
 *
 * `usePredict` is a mutation, not a query — like `useScenario`'s identical use
 * of it, its shared `data`/`error`/`isPending` are not keyed by input, so two
 * calls in flight together (the driver or delta changed again before the
 * first answer lands) settle in whatever order the network delivers them, and
 * the mutation object just reflects whichever one settled LAST. A slow,
 * already-superseded response landing after a fast, current one would
 * silently overwrite it. `requestIdRef` guards against exactly that, mirroring
 * `useScenario`'s `useDebouncedPredict`: each fired request is stamped with a
 * token, and its `onSuccess`/`onError` is only applied if that token is still
 * the latest — a later edit (or a clear) bumps the ref first, so a superseded
 * response is dropped on arrival instead of overwriting `impacts`/`error`, and
 * `isPending` can't be revived by a response for a request the user has
 * already moved past either.
 */
export function WorldModelWhatIf({
  drivers,
  target
}: {
  drivers: SensitivityDriver[];
  target: string;
}) {
  // Default to the highest-leverage driver that is actually quantified — a
  // driver with no coefficient cannot propagate anything.
  const quantified = useMemo(
    () => drivers.filter((d) => d.effective_coefficient != null),
    [drivers]
  );
  const [measure, setMeasure] = useState<string>(() => quantified[0]?.measure ?? "");
  const [deltaText, setDeltaText] = useState("1");

  const predict = usePredict();
  const { mutate: runPredict, reset: resetPredict } = predict;
  const selected = quantified.find((d) => d.measure === measure) ?? quantified[0];
  const selectedMeasure = selected?.measure;
  const delta = Number(deltaText);
  const runnable = !!selectedMeasure && Number.isFinite(delta) && delta !== 0;

  const [result, setResult] = useState<WhatIfResult>(IDLE_RESULT);
  const requestIdRef = useRef(0);

  // Live propagation: fire whenever the driver or delta settles. An empty or
  // zero delta clears the stale result rather than leaving the last run
  // showing, and bumps the request id so an in-flight response for the
  // abandoned input cannot rehydrate state the user has already dismissed.
  useEffect(() => {
    if (!selectedMeasure || !Number.isFinite(delta) || delta === 0) {
      requestIdRef.current++;
      resetPredict();
      setResult(IDLE_RESULT);
      return;
    }
    const handle = window.setTimeout(() => {
      const requestId = ++requestIdRef.current;
      setResult((prev) => ({ ...prev, isPending: true }));
      runPredict(
        { changes: [{ measure: selectedMeasure, delta }], values: undefined },
        {
          onSuccess: (data) => {
            // Superseded by a later edit while this was in flight — a stale
            // result must not clobber the current one, or set/clear it either.
            if (requestId !== requestIdRef.current) return;
            setResult({ impacts: data.impacts, error: null, isPending: false });
          },
          onError: (error) => {
            if (requestId !== requestIdRef.current) return;
            setResult({ impacts: undefined, error, isPending: false });
          }
        }
      );
    }, DEBOUNCE_MS);
    return () => window.clearTimeout(handle);
  }, [selectedMeasure, delta, runPredict, resetPredict]);

  const impact = result.impacts?.find((i) => i.measure === target);

  if (quantified.length === 0) {
    return (
      <p className='font-mono text-[9.5px] text-muted-foreground leading-relaxed'>
        No driver carries a coefficient, so a what-if cannot be propagated.
      </p>
    );
  }

  return (
    <div className='flex flex-col gap-1.5 pt-1'>
      <div className='flex items-center gap-1'>
        <span className='min-w-0 flex-1'>
          <SectionHeader title='What-if' subtitle='updates live' />
        </span>
        <InfoTip
          content={`Moves the selected driver by Δ, in the driver's own units, and propagates the modelled effect up to ${shortMeasureName(
            target
          )}. A structural estimate from the metric tree, not a forecast.`}
        />
      </div>
      <div className='flex items-center gap-1.5'>
        <span className='shrink-0 font-mono text-muted-foreground text-xs'>Δ</span>
        <Select value={selectedMeasure ?? ""} onValueChange={setMeasure}>
          <SelectTrigger size='sm' className='h-7 flex-1 font-mono text-xs'>
            <SelectValue />
          </SelectTrigger>
          <SelectContent className={DESIGN_REFERENCE}>
            {quantified.map((d) => (
              <SelectItem key={d.measure} value={d.measure} className='font-mono text-xs'>
                {shortMeasureName(d.measure)}
              </SelectItem>
            ))}
          </SelectContent>
        </Select>
        <span className='shrink-0 font-mono text-muted-foreground text-xs'>by</span>
        <input
          type='number'
          value={deltaText}
          onChange={(e) => setDeltaText(e.target.value)}
          aria-label='Driver delta'
          data-testid='wm-whatif-delta'
          className='h-7 w-16 shrink-0 border border-input bg-transparent px-1.5 font-mono text-xs'
        />
      </div>

      {selected?.form && (
        <p className='font-mono text-[9.5px] text-muted-foreground'>
          {shortMeasureName(selected.measure)} relates to {shortMeasureName(target)} as{" "}
          <span className='text-foreground'>{selected.form}</span>
          {FORM_HELP[selected.form] ? <InfoTip content={FORM_HELP[selected.form]} /> : null}
        </p>
      )}

      {runnable && result.isPending && <SectionSpinner label='propagating…' />}

      {result.error && (
        <p className='font-mono text-[10px] text-destructive leading-relaxed'>
          {result.error instanceof Error ? result.error.message : "Failed to run the what-if."}
        </p>
      )}

      {runnable &&
        result.impacts &&
        (impact ? (
          <Row className='justify-between'>
            <span className='truncate text-muted-foreground'>{shortMeasureName(target)}</span>
            <span className='flex shrink-0 items-center gap-1.5'>
              <span className='text-info'>{formatDelta(impact.estimated_delta)}</span>
              {impact.form && <MetaBadge tooltip={FORM_HELP[impact.form]}>{impact.form}</MetaBadge>}
              <MetaBadge tooltip={CONFIDENCE_HELP}>{impact.confidence}</MetaBadge>
            </span>
          </Row>
        ) : (
          <p className='font-mono text-[9.5px] text-muted-foreground leading-relaxed'>
            That move does not propagate to {shortMeasureName(target)}.
          </p>
        ))}
    </div>
  );
}
