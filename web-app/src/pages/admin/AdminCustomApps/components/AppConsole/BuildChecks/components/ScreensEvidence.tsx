import { ChevronRight } from "lucide-react";
import { cn } from "@/libs/shadcn/utils";
import { ADMIN_TONE } from "@/pages/admin/components/adminTone";
import type { BuildCheckDetail } from "@/types/appBuildChecks";
import {
  type ScreenReading,
  screensLine,
  smokeScreens,
  type UnlistedReading,
  unlistedScreens
} from "../screensModel";
import { StatusIcon } from "./StatusIcon";

/**
 * Every screen the runner opened, one line each: its path, then each viewport as a
 * glyph. A screen that failed comes first and carries what it threw under it; a
 * request that failed, a write the checker was refused or a request to another site it
 * did not send is said there too, and is a warning only. Above them, closed until asked: the screens those link to that were not
 * in the list, so "every screen" is not read as more than the check covered.
 */
export const ScreensEvidence = ({ detail }: { detail: BuildCheckDetail }) => {
  const screens = smokeScreens(detail);
  if (screens.length === 0) return null;
  const unlisted = unlistedScreens(detail);
  return (
    <div className='flex flex-col gap-1' data-testid='build-check-screens'>
      <span className='text-muted-foreground text-xs' data-testid='build-check-screens-count'>
        {screensLine(detail, screens.length)}
      </span>
      {unlisted && <UnlistedScreens unlisted={unlisted} />}
      <ul className='flex max-h-96 flex-col overflow-y-auto'>
        {screens.map((screen) => (
          <ScreenLine key={screen.path} screen={screen} />
        ))}
      </ul>
    </div>
  );
};

/** The linked screens no check opened: a count, and on a click which, and how to list them. */
const UnlistedScreens = ({ unlisted }: { unlisted: UnlistedReading }) => {
  const more = unlisted.total - unlisted.names.length;
  return (
    <details className='group text-xs' data-testid='build-check-screens-unlisted'>
      <summary
        className={cn("flex cursor-pointer list-none items-center gap-1.5", ADMIN_TONE.warn.text)}
      >
        <ChevronRight className='size-3 transition-transform group-open:rotate-90' />
        {unlisted.total} linked screen{unlisted.total === 1 ? "" : "s"} not checked
      </summary>
      <div className='flex flex-col gap-1 pt-1 pl-5'>
        <span className='text-muted-foreground'>
          The screens below link to these, and the list does not hold them. Declare the ones that
          matter in the app&rsquo;s oxy-app.json, under checks.screens.
        </span>
        <ul className='flex max-h-40 flex-col overflow-y-auto font-mono'>
          {unlisted.names.map((name) => (
            <li key={name} className='truncate' title={name}>
              {name}
            </li>
          ))}
        </ul>
        {more > 0 && <span className='text-muted-foreground'>and {more} more</span>}
      </div>
    </details>
  );
};

const ScreenLine = ({ screen }: { screen: ScreenReading }) => (
  <li
    className='flex flex-col gap-0.5 border-border/40 border-b py-1 text-xs last:border-b-0'
    data-testid='build-check-screen'
    data-path={screen.path}
    data-failing={screen.failing}
  >
    <div className='flex items-center gap-3'>
      <span className='min-w-0 flex-1 truncate font-mono' title={screen.name}>
        {screen.name}
      </span>
      {screen.viewports.map((v) => (
        <span
          key={v.name}
          className='inline-flex shrink-0 items-center gap-1'
          title={v.text}
          data-testid={`build-check-screen-${v.name}`}
          data-ok={v.ok}
        >
          <StatusIcon status={v.ok ? "passed" : "failed"} />
          <span className={v.ok ? "text-muted-foreground" : ADMIN_TONE.danger.text}>
            {v.name}
            {/* What threw is under the line; never getting ready has no line of its own. */}
            {v.ready ? <span className='sr-only'> {v.text}</span> : ` ${v.text}`}
          </span>
        </span>
      ))}
    </div>
    {screen.errors.map((e) => (
      <Under key={e.value} at={e.viewports} testId='build-check-screen-error'>
        <span className={cn("min-w-0 break-words font-mono", ADMIN_TONE.danger.text)}>
          {e.value}
        </span>
      </Under>
    ))}
    {screen.failedRequests.map((r) => (
      <Under
        key={`${r.value.status}:${r.value.url}`}
        at={r.viewports}
        testId='build-check-screen-request'
      >
        <span className={cn("shrink-0 font-medium font-mono", ADMIN_TONE.warn.text)}>
          {r.value.status ?? r.value.error ?? "failed"}
        </span>
        <span className='min-w-0 truncate font-mono text-muted-foreground' title={r.value.url}>
          {r.value.url}
        </span>
      </Under>
    ))}
    {screen.refusedWrites > 0 && (
      <span
        className={cn("pl-5", ADMIN_TONE.warn.text)}
        title={screen.refused.join("\n") || undefined}
        data-testid='build-check-screen-refused'
      >
        {screen.refusedWrites} write{screen.refusedWrites === 1 ? "" : "s"} refused
      </span>
    )}
    {screen.offOrigin.length > 0 && (
      <span
        className={cn("pl-5", ADMIN_TONE.warn.text)}
        title={screen.offOrigin.join("\n")}
        data-testid='build-check-screen-off-origin'
      >
        {screen.offOrigin.length} request{screen.offOrigin.length === 1 ? "" : "s"} to another site
        not sent
      </span>
    )}
  </li>
);

/** One thing a screen showed, under its line: at which viewports, then what. */
const Under = ({
  at,
  testId,
  children
}: {
  at: string[];
  testId: string;
  children: React.ReactNode;
}) => (
  <div className='flex min-w-0 items-baseline gap-2 pl-5' data-testid={testId}>
    <span className='shrink-0 text-muted-foreground'>{at.join(" ")}</span>
    {children}
  </div>
);
