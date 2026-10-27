import { Alert, AlertDescription, AlertTitle } from "@/components/ui/shadcn/alert";
import { cn } from "@/libs/shadcn/utils";
import { TONE_SURFACE, ToneBadge, ToneIcon } from "@/pages/ide/Experiments/components/glance/tone";
import { formatInstant } from "@/pages/ide/Experiments/dates";
import { effectFormat } from "@/pages/ide/Experiments/format";
import { grainOf } from "@/pages/ide/Experiments/grain";
import type { ResultResponse } from "@/types/experiment";
import VerbatimRefusal from "../../../VerbatimRefusal";
import AbortLeversNote from "../AbortLeversNote";
import HowCalculated from "../HowCalculated";
import { alphaInWords, caveatBadges, plainVerdict, runBadge, verdictTone } from "./badges";
import HeadlineText from "./HeadlineText";
import { HINTS, Hinted, HintIcon } from "./hints";
import StatTiles from "./StatTiles";

/** `+$18.20/unit-day` → a large figure and a small unit, as one text node pair. */
function BigFigure({ text, testId }: { text: string; testId: string }) {
  const slash = text.indexOf("/");
  const [figure, unit] = slash === -1 ? [text, ""] : [text.slice(0, slash), text.slice(slash)];
  return (
    <p className='font-semibold text-4xl tabular-nums tracking-tight' data-testid={testId}>
      {figure}
      {unit && <span className='ml-1 font-normal text-base text-muted-foreground'>{unit}</span>}
    </p>
  );
}

/**
 * The server's label without its shouted lead (`RETROSPECTIVE — `, `ABORTED RUN — `):
 * the band's title already says which it is. The rest starts with a capital.
 */
export function bandText(label: string): string {
  const rest = label.replace(/^[A-Z][A-Z ]+ — /, "");
  return rest.charAt(0).toUpperCase() + rest.slice(1);
}

/**
 * An aborted or retrospective run says so first, in the server's own label, on a
 * coloured band across the top of the card: neither is the registered result.
 */
function StandingBand({ result }: { result: ResultResponse }) {
  const { report } = result;
  const kind = report.status.kind;
  if (kind === "registered") return null;
  const badge = runBadge(kind);
  if (kind === "interim") {
    return (
      <div
        className={cn(
          "flex items-start gap-2 border-b px-5 py-3 text-sm",
          TONE_SURFACE[badge.tone]
        )}
        data-testid='experiment-report-interim'
      >
        <ToneIcon tone={badge.tone} className='mt-0.5 size-4' />
        <p>{report.label}</p>
      </div>
    );
  }
  return (
    <div
      className={cn("flex items-start gap-2 border-b px-5 py-3 text-sm", TONE_SURFACE[badge.tone])}
      data-testid={
        kind === "aborted" ? "experiment-report-abort" : "experiment-report-retrospective"
      }
    >
      <ToneIcon tone={badge.tone} className='mt-0.5 size-4' />
      <div>
        <p className='font-semibold'>
          {kind === "aborted" ? "Aborted run" : "Retrospective analysis"}
        </p>
        <p>{bandText(report.label)}</p>
        {report.status.kind === "aborted" && (
          <AbortLeversNote kept={report.status.levers_kept === true} />
        )}
      </div>
    </div>
  );
}

/**
 * The report at a glance: the run's standing, the verdict and the gross effect in
 * large type, every caveat as a coloured pill, then the key numbers as tiles. All
 * words are the server's or plain labels; colour is the only thing added here.
 */
export default function ReportSummary({ result }: { result: ResultResponse }) {
  const { report } = result;
  const h = report.headline;
  const fmt = effectFormat(report.design, grainOf(report.grain), { share: report.share });
  const run = runBadge(report.status.kind);
  const refused = report.refusal !== null;
  return (
    <section
      className='overflow-hidden rounded-lg border bg-card'
      data-testid='experiment-report-summary'
      data-kind={report.status.kind}
    >
      <StandingBand result={result} />
      <div className='flex flex-col gap-4 p-5' data-testid='experiment-report-effect'>
        <div className='flex flex-wrap items-center gap-1.5' data-testid='experiment-report-badges'>
          {report.status.kind === "registered" ? (
            <span data-testid='experiment-report-label'>
              <ToneBadge tone={run.tone} testId='experiment-badge-run'>
                {report.label}
              </ToneBadge>
            </span>
          ) : report.status.kind === "interim" ? null : (
            <ToneBadge tone={run.tone} testId='experiment-badge-run'>
              {run.label}
            </ToneBadge>
          )}
          {refused ? (
            <ToneBadge tone='danger' testId='experiment-badge-refused'>
              Estimator refused
            </ToneBadge>
          ) : (
            h && (
              <Hinted hint={`${alphaInWords(report.alpha)}. The engine's verdict: ${h.verdict}.`}>
                <span className='inline-flex cursor-help'>
                  <ToneBadge tone={verdictTone(report)} testId='experiment-report-verdict-badge'>
                    {plainVerdict(report)}
                  </ToneBadge>
                </span>
              </Hinted>
            )
          )}
          {caveatBadges(report).map((b) => (
            <ToneBadge key={b.id} tone={b.tone} testId={`experiment-badge-${b.id}`}>
              {b.label}
            </ToneBadge>
          ))}
        </div>

        {refused ? (
          <VerbatimRefusal
            title='The estimator refused'
            text={report.refusal ?? ""}
            testId='experiment-report-refusal'
          />
        ) : h ? (
          <div className='flex flex-col gap-1'>
            <p className='flex items-center gap-1 text-muted-foreground text-xs'>
              Gross effect
              <HintIcon
                hint={HINTS.estimate(
                  `${report.design === "groups" ? "group" : "unit"}-${grainOf(report.grain)}`
                )}
                testId='experiment-report-estimate-hint'
              />
            </p>
            <BigFigure text={fmt.signed(h.estimate)} testId='experiment-report-estimate' />
            <p className='text-sm'>
              <span className='inline-flex items-center gap-1 text-muted-foreground'>
                Confidence interval
                <HintIcon hint={HINTS.interval} testId='experiment-report-interval-hint' />
              </span>{" "}
              <span className='font-medium tabular-nums' data-testid='experiment-report-interval'>
                {fmt.interval(h.interval.low, h.interval.high)}
              </span>
            </p>
            <HeadlineText text={h.text} />
          </div>
        ) : (
          <Alert>
            <AlertTitle>No headline was produced</AlertTitle>
            <AlertDescription>The report carries no estimate and no refusal.</AlertDescription>
          </Alert>
        )}

        <StatTiles report={report} />

        {report.direction.mismatch && report.direction.message && (
          <Alert data-testid='experiment-report-direction-mismatch'>
            <AlertTitle>Direction differs from the prediction</AlertTitle>
            <AlertDescription>{report.direction.message}</AlertDescription>
          </Alert>
        )}

        {report.projection?.warning && (
          <Alert variant='destructive' data-testid='experiment-report-bias-warning'>
            <AlertTitle>Likely direction of bias in the projection</AlertTitle>
            <AlertDescription>{report.projection.warning}</AlertDescription>
          </Alert>
        )}

        {report.lever_note && (
          <Alert data-testid='experiment-lever-changed'>
            <AlertTitle>The lever's definition changed mid-run</AlertTitle>
            <AlertDescription>{report.lever_note}</AlertDescription>
          </Alert>
        )}

        <HowCalculated>
          {h && !refused && (
            <p>
              Verdict: <span data-testid='experiment-report-verdict'>{h.verdict}</span> (
              {alphaInWords(report.alpha)}).
            </p>
          )}
          <p data-testid='experiment-report-estimand'>Estimand: {report.estimand}.</p>
          {report.treated_total && (
            <p data-testid='experiment-report-total-basis'>
              Total over treated units: {report.treated_total.basis}
            </p>
          )}
          {report.projection && (
            <p data-testid='experiment-report-projection-assumption'>
              {[report.projection.label, report.projection.assumption]
                .filter(Boolean)
                .map((t) => t.replace(/\.?$/, "."))
                .join(" ")}
            </p>
          )}
          <p>
            Design <code className='font-mono'>{result.design_id.slice(0, 8)}</code>,{" "}
            {report.status.kind === "interim"
              ? `read through ${report.status.through}`
              : `reported ${formatInstant(result.created_at)}`}
            .
          </p>
          <p>Hypothesis: {report.hypothesis}</p>
        </HowCalculated>
      </div>
    </section>
  );
}
