import { cn } from "@/libs/shadcn/utils";
import { ADMIN_TONE } from "@/pages/admin/components/adminTone";
import { type AppMap, TARGET_GROUP_LABEL } from "../appMapModel";
import { entryText, runsLabel, touchText } from "../describe";

/**
 * The app map as text: per function, what starts it and what it touched. What a
 * screen reader reads in place of the diagram, and what a narrow column shows
 * instead of one. Function names are the same control as the diagram's nodes.
 */
export const AppMapList = ({
  map,
  onOpenFunction
}: {
  map: AppMap;
  onOpenFunction: (name: string) => void;
}) => {
  const targets = new Map(map.targets.map((t) => [t.id, t]));
  return (
    <ul className='flex flex-col gap-2.5 text-xs' data-testid='admin-app-map-list'>
      {map.functions.map((fn) => {
        const entries = map.entries.filter((e) => e.fn === fn.name);
        const touches = map.edges.filter((e) => e.kind === "touch" && e.from === fn.id);
        return (
          <li key={fn.id} data-testid={`admin-app-map-list-${fn.name}`}>
            <div className='flex flex-wrap items-baseline gap-x-2'>
              <button
                type='button'
                onClick={() => onOpenFunction(fn.name)}
                className='font-medium font-mono underline-offset-2 hover:underline'
              >
                {fn.name}
              </button>
              <span className='text-muted-foreground'>{runsLabel(fn)}</span>
            </div>
            <p className='text-muted-foreground'>
              Started by:{" "}
              <span className='text-foreground'>{entries.map(entryText).join("; ")}</span>
            </p>
            {touches.length === 0 ? (
              <p className='text-muted-foreground'>
                {fn.runs === null ? "Touches: not read yet." : "Touches: nothing recorded."}
              </p>
            ) : (
              <div className='text-muted-foreground'>
                Touches:
                <ul className='ml-4 list-disc'>
                  {touches.map((edge) => {
                    const target = targets.get(edge.to);
                    if (!target) return null;
                    return (
                      <li
                        key={edge.id}
                        className={cn(edge.failedCalls > 0 && ADMIN_TONE.danger.text)}
                      >
                        <span className={cn(edge.failedCalls === 0 && "text-foreground")}>
                          {TARGET_GROUP_LABEL[target.group]}{" "}
                          <span className='font-mono'>{target.label}</span>
                        </span>{" "}
                        ({touchText(edge)})
                      </li>
                    );
                  })}
                </ul>
              </div>
            )}
          </li>
        );
      })}
    </ul>
  );
};
