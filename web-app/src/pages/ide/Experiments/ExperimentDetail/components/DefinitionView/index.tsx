import { ChevronRight } from "lucide-react";
import {
  Collapsible,
  CollapsibleContent,
  CollapsibleTrigger
} from "@/components/ui/shadcn/collapsible";
import type { ExperimentListItem } from "@/types/experiment";
import { isEditable } from "../ExperimentMenu";
import {
  comparesText,
  confidenceText,
  directionText,
  measureLabel,
  plural,
  randomisesText
} from "./describe";
import GuardrailList from "./GuardrailList";
import LeverCard from "./LeverCard";
import OtherSettings from "./OtherSettings";
import OutcomeCard from "./OutcomeCard";
import ScheduleCard from "./ScheduleCard";
import { readDefinition } from "./safe";
import { Block } from "./shared";

const KNOWN = new Set([
  "experiment",
  "hypothesis",
  "owner",
  "for_each",
  "assign_by",
  "lever",
  "design",
  "outcome",
  "substitution_check",
  "guardrails"
]);

/** One line for the folded header: the measure, direction and confidence. */
function summaryLine(def: ReturnType<typeof readDefinition>): string {
  const o = def.outcome ?? {};
  return [
    o.target ? measureLabel(o.target) : null,
    o.direction ? directionText(o.direction) : null,
    typeof o.alpha === "number" ? confidenceText(o.alpha) : null,
    def.design?.waves?.length ? plural(def.design.waves.length, "wave") : null
  ]
    .filter(Boolean)
    .join(" · ");
}

/**
 * The compiled definition at a glance: key figures as tiles, long parts folded
 * (but mounted). Open for a draft, whose design is still being worked on; folded
 * to one line otherwise. Unknown keys land in Other settings.
 */
export default function DefinitionView({ summary }: { summary: ExperimentListItem }) {
  const raw = summary.definition as unknown;
  const def = readDefinition(raw);
  if (!raw) {
    return (
      <section className='rounded-lg border p-4' data-testid='experiment-definition'>
        <h3 className='mb-2 font-medium text-sm'>Definition</h3>
        <p className='text-muted-foreground text-sm'>No definition to show.</p>
      </section>
    );
  }
  const extra = Object.fromEntries(Object.entries(def).filter(([k]) => !KNOWN.has(k)));
  const noun = def.assign_by ?? def.for_each ?? "unit";
  // A run read from past data, or a draft whose split is declared rather than
  // drawn, was never randomised or priced, and has no seed.
  const past = summary.retrospective || def.design?.assignment === "observed";
  const assignText = past ? comparesText : randomisesText;
  return (
    <Collapsible asChild defaultOpen={isEditable(summary.status)}>
      <section className='group/def rounded-lg border p-4' data-testid='experiment-definition'>
        <CollapsibleTrigger className='flex w-full items-center gap-2 text-left'>
          <ChevronRight className='size-4 shrink-0 transition-transform group-data-[state=open]/def:rotate-90' />
          <span className='font-medium text-sm'>Definition</span>
          <span className='min-w-0 truncate text-muted-foreground text-xs group-data-[state=open]/def:hidden'>
            {summaryLine(def)}
          </span>
        </CollapsibleTrigger>
        <CollapsibleContent forceMount className='data-[state=closed]:hidden'>
          <div className='mt-3 space-y-4'>
            <Block id='experiment' title='What it tests'>
              <p className='font-medium text-sm'>{def.hypothesis ?? "—"}</p>
              <p className='text-muted-foreground text-xs'>
                {def.for_each ? `${assignText(def.for_each, def.assign_by ?? null)}. ` : ""}
                Owned by {def.owner ?? "—"}.
              </p>
            </Block>
            <OutcomeCard def={def} past={past} />
            <LeverCard lever={def.lever} />
            <ScheduleCard design={def.design} noun={noun} past={past} />
            <GuardrailList def={def} />
            <OtherSettings extra={extra} />
          </div>
        </CollapsibleContent>
      </section>
    </Collapsible>
  );
}
