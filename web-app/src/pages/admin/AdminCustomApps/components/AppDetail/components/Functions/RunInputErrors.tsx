import { ADMIN_TONE } from "@/pages/admin/components/adminTone";

/** How many errors are listed before "and N more". */
const SHOWN = 3;

/**
 * Why Run now did not send the params box — invalid JSON, or where it does not
 * fit the function's input schema — and, for a schema refusal, a way past it:
 * the schema is advisory when the function runs, and a stale one must not stand
 * between an operator and the function.
 */
export const RunInputErrors = ({
  errors,
  canRunAnyway,
  onRunAnyway,
  pending
}: {
  errors: string[];
  canRunAnyway: boolean;
  onRunAnyway: () => void;
  pending: boolean;
}) => {
  const more = errors.length - SHOWN;
  return (
    <div className='flex flex-col gap-1' data-testid='admin-app-function-input-errors' role='alert'>
      <ul className={`flex flex-col gap-0.5 font-mono text-xs ${ADMIN_TONE.danger.text}`}>
        {errors.slice(0, SHOWN).map((error) => (
          <li key={error} className='whitespace-pre-wrap break-words'>
            {error}
          </li>
        ))}
        {more > 0 && <li className='text-muted-foreground'>and {more} more</li>}
      </ul>
      {canRunAnyway && (
        <button
          type='button'
          disabled={pending}
          onClick={onRunAnyway}
          className='self-start text-muted-foreground text-xs underline underline-offset-2 hover:text-foreground'
          data-testid='admin-app-function-run-anyway'
        >
          Run anyway — the schema is not enforced when the function runs
        </button>
      )}
    </div>
  );
};
