import { ChevronDown, ChevronRight, Clock, Code2, KeyRound } from "lucide-react";
import { Badge } from "@/components/ui/shadcn/badge";
import { useAppFunctions } from "@/hooks/api/customApps/useAppFunctions";
import { AdminAsync } from "@/pages/admin/components/AdminAsync";
import type { AppFunctionSummary } from "@/types/apps";
import { FunctionDetail } from "./FunctionDetail";
import type { AppRef } from "./InvocationRow";

/**
 * The AppDetail "Functions" section: the app's Oxy Functions in its active
 * build, each expandable to its manifest config, recent invocation history,
 * and a "Run now" trigger that surfaces the resulting job run's logs. Manage +
 * debug the code-first functions shipped via `oxyc publish`.
 */
export const Functions = ({
  appId,
  selected,
  onSelect,
  environment,
  appUrl,
  appRef
}: {
  appId: string;
  /** When given, a failed invocation offers "Copy for an agent". */
  appRef?: AppRef;
  /** `?fn=` from the admin URL — the open function, or none. */
  selected: string | null;
  onSelect: (name: string | null) => void;
  /** The environment whose build is listed and where Run now runs. `undefined`
   *  is production, the server's own default. */
  environment?: string;
  /** The app's canonical URL (`<base>/customer-apps/<org>/<app>/`), which a
   *  route function is served under as `fn/<name>`. Omit where it is unknown. */
  appUrl?: string;
}) => {
  const functions = useAppFunctions(appId, environment);

  return (
    <AdminAsync
      query={functions}
      noun='this app&rsquo;s functions'
      rows={2}
      isEmpty={(fns) => fns.length === 0}
      empty={
        <p className='text-muted-foreground text-xs' data-testid='admin-app-functions-empty'>
          This app ships no Oxy Functions. Add a <code>functions/&lt;name&gt;.ts</code> to the
          bundle and <code>oxyc publish</code>.
        </p>
      }
    >
      {(fns) => (
        <ul className='flex flex-col gap-1.5' data-testid='admin-app-functions-list'>
          {fns.map((fn) => (
            <FunctionRow
              key={fn.name}
              appId={appId}
              fn={fn}
              selected={selected}
              onSelect={onSelect}
              environment={environment}
              appUrl={appUrl}
              appRef={appRef}
            />
          ))}
        </ul>
      )}
    </AdminAsync>
  );
};

/**
 * One function. Open/closed is the URL's (`?fn=<name>`), not this row's, so a
 * link to a function opens it and Back closes it again — the same rule the rest
 * of this surface follows. Only one is open at a time, which is what a single
 * `?fn=` can express and what an operator reading one function wants anyway.
 */
const FunctionRow = ({
  appId,
  fn,
  selected,
  onSelect,
  environment,
  appUrl,
  appRef
}: {
  appId: string;
  appRef?: AppRef;
  fn: AppFunctionSummary;
  selected: string | null;
  onSelect: (name: string | null) => void;
  environment?: string;
  appUrl?: string;
}) => {
  const open = selected === fn.name;
  const setOpen = () => onSelect(open ? null : fn.name);
  const Chevron = open ? ChevronDown : ChevronRight;
  return (
    <li
      className='rounded-md border border-border bg-card'
      data-testid={`admin-app-function-${fn.name}`}
    >
      <button
        type='button'
        onClick={setOpen}
        className='flex w-full items-center gap-2 px-3 py-2 text-left'
        aria-expanded={open}
      >
        <Chevron className='size-3.5 shrink-0 text-muted-foreground' />
        <Code2 className='size-3.5 shrink-0 text-vis-violet' />
        <span className='truncate font-mono text-xs'>{fn.name}</span>
        <div className='ml-auto flex shrink-0 items-center gap-1'>
          <SurfaceBadges fn={fn} />
        </div>
      </button>
      {open && (
        <FunctionDetail
          appId={appId}
          fn={fn}
          environment={environment}
          appUrl={appUrl}
          appRef={appRef}
        />
      )}
    </li>
  );
};

/** Which invocation surfaces the function declares — the at-a-glance identity. */
const SurfaceBadges = ({ fn }: { fn: AppFunctionSummary }) => (
  <>
    {fn.route && (
      <Badge variant='secondary' className='h-5'>
        Route
      </Badge>
    )}
    {fn.schedule && (
      <Badge variant='secondary' className='h-5 gap-1'>
        <Clock className='size-3' />
        Scheduled
      </Badge>
    )}
    {fn.airway && (
      <Badge variant='secondary' className='h-5'>
        Airway
      </Badge>
    )}
    {fn.check && (
      <Badge
        variant='secondary'
        className='h-5'
        title='Marked "check": true — run by oxyc checks run'
      >
        Check
      </Badge>
    )}
    {fn.secrets_write && (
      <Badge variant='secondary' className='h-5 gap-1' title='Writes app-scoped secrets'>
        <KeyRound className='size-3' />
        Secrets
      </Badge>
    )}
  </>
);
