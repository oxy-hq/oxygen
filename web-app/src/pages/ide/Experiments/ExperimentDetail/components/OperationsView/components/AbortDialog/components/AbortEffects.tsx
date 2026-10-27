import { HintIcon } from "@/pages/ide/Experiments/components/glance/Hinted";
import type { AbortLevers } from "@/types/experiment";

/** Enough unit names to recognise; the rest are counted. */
const NAMED = 5;

export function unitList(units: string[]): string {
  if (units.length <= NAMED) return units.join(", ");
  return `${units.slice(0, NAMED).join(", ")} and ${units.length - NAMED} more`;
}

function leverLine(levers: AbortLevers, unitsOn: string[]): { text: string; hint: string } {
  if (levers === "keep") {
    return {
      text: "Leaves every lever as it is",
      hint: "No off automation runs. Units that are on stay on, and units not switched on yet are never switched on."
    };
  }
  const hint =
    "Runs the design's off automation at each unit that is on. Units not switched on yet are never switched on.";
  if (unitsOn.length === 0) return { text: "No lever is on yet, so none is switched", hint };
  const n = unitsOn.length;
  const units = n === 1 ? "1 unit that is on now" : `${n} units that are on now`;
  return { text: `Turns the lever off at ${units}: ${unitList(unitsOn)}`, hint };
}

/** One short line per consequence of the abort, each with its detail behind an ⓘ. */
export default function AbortEffects({
  levers,
  unitsOn,
  owner
}: {
  levers: AbortLevers;
  unitsOn: string[];
  owner: string;
}) {
  const lines = [
    { id: "levers", ...leverLine(levers, unitsOn) },
    {
      id: "owner",
      text: `Notifies ${owner}`,
      hint: "The owner is told the reason you give below."
    },
    {
      id: "report",
      text: "The report is labelled Aborted and uses data up to today",
      hint: "The measurement stops at the start of the day the abort takes effect, so that day and later are not read. It is never presented as the registered result."
    },
    {
      id: "undo",
      text: "This can't be undone",
      hint: "To run it again, duplicate the experiment and register the copy."
    }
  ];
  return (
    <ul className='flex flex-col gap-1.5 text-sm' data-testid='experiment-abort-effects'>
      {lines.map((l) => (
        <li key={l.id} className='flex items-center gap-1.5'>
          <span>{l.text}</span>
          <HintIcon hint={l.hint} testId={`experiment-abort-hint-${l.id}`} />
        </li>
      ))}
    </ul>
  );
}
