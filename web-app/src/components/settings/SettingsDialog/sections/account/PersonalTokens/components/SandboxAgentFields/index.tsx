import { ArrowDown, ArrowUp, CornerDownLeft } from "lucide-react";
import type React from "react";
import { useId } from "react";
import { KeyHint } from "@/components/ui/KeyHint";
import type { SandboxAgentForm } from "../../useSandboxAgentForm";
import SheetRow from "../SheetRow";
import SandboxAppPicker from "./components/SandboxAppPicker";

type Props = Pick<SandboxAgentForm, "apps" | "picked" | "limits" | "refusal" | "toggle">;

/** "2 apps": the number carries the line, so it is set a step heavier than the words. */
const Count: React.FC<{ count: number }> = ({ count }) => (
  <>
    <b className='font-medium text-foreground'>{count}</b> app{count === 1 ? "" : "s"}
  </>
);

/**
 * The "Sandbox agent" type's own row of the sheet, in place of the workspace picker: the apps
 * the token names, how many are picked, and a refusal from the server beside the picks it is
 * about.
 *
 * In a window too short for the whole form this is the row that gives up height: the list of
 * apps scrolls inside it, so the fields around it and the buttons stay where they are.
 */
const SandboxAgentFields: React.FC<Props> = ({ apps, picked, limits, refusal, toggle }) => {
  const label = useId();
  const keys = useId();
  const full = picked.length >= limits.max_apps;
  return (
    <SheetRow
      label='Apps'
      labelId={label}
      className='min-h-44 shrink'
      contentClassName='flex min-h-0 flex-col'
    >
      {/* biome-ignore lint/a11y/useSemanticElements: a fieldset can't sit on the sheet's value edge beside its own label. */}
      <div
        role='group'
        aria-labelledby={label}
        className='flex min-h-0 flex-col'
        data-testid='account-token-sandbox-fields'
      >
        <SandboxAppPicker
          apps={apps}
          pickedIds={picked.map((app) => app.id)}
          full={full}
          onToggle={toggle}
          keysHintId={keys}
        />
        <div className='mt-1 flex min-h-7 shrink-0 flex-wrap items-center justify-between gap-x-4 gap-y-1 px-3 text-muted-foreground'>
          <p data-testid='account-token-sandbox-count'>
            {picked.length === 0 ? (
              `Pick at least one app, and up to ${limits.max_apps}.`
            ) : full ? (
              <>
                <Count count={picked.length} /> picked, the most one token covers. Untick one to
                pick another.
              </>
            ) : (
              <>
                <Count count={picked.length} /> picked, of up to {limits.max_apps}.
              </>
            )}
          </p>
          <p className='hidden items-center gap-1.5 text-xs sm:flex' aria-hidden='true'>
            <KeyHint>
              <ArrowUp className='size-3' />
            </KeyHint>
            <KeyHint>
              <ArrowDown className='size-3' />
            </KeyHint>
            move
            <KeyHint className='ml-1.5'>
              <CornerDownLeft className='size-3' />
            </KeyHint>
            pick
          </p>
          <span className='sr-only' id={keys}>
            In the search field, the arrow keys move through the apps and Enter picks the
            highlighted one.
          </span>
        </div>
        {refusal && (
          <p
            className='mt-1 shrink-0 px-3 text-destructive'
            role='alert'
            data-testid='account-token-sandbox-error'
          >
            {refusal}
          </p>
        )}
      </div>
    </SheetRow>
  );
};

export default SandboxAgentFields;
