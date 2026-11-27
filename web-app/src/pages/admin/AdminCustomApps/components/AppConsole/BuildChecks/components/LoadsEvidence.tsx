import { cn } from "@/libs/shadcn/utils";
import { ADMIN_TONE } from "@/pages/admin/components/adminTone";
import type { BuildCheckDetail, BuildCheckViewport } from "@/types/appBuildChecks";
import { readyText, viewportName } from "../buildChecksModel";

/**
 * The staging host opened in a headless browser, once per viewport: whether the app
 * said it was ready and how fast, what threw on the page, and which requests failed.
 */
export const LoadsEvidence = ({ detail }: { detail: BuildCheckDetail }) => (
  <div className='flex flex-col gap-2' data-testid='build-check-loads'>
    {detail.url && (
      <span className='truncate text-muted-foreground text-xs'>
        <a
          href={detail.url}
          target='_blank'
          rel='noreferrer'
          className='font-mono hover:underline'
          title={detail.url}
        >
          {detail.url}
        </a>
        {detail.identity && <span> · as {detail.identity}</span>}
      </span>
    )}
    <div className='grid gap-2 sm:grid-cols-2'>
      {(detail.viewports ?? []).map((v) => (
        <Viewport key={v.name} v={v} />
      ))}
    </div>
  </div>
);

const Viewport = ({ v }: { v: BuildCheckViewport }) => (
  <div
    className='flex min-w-0 flex-col gap-1 rounded-sm border border-border/60 px-2 py-1.5 text-xs'
    data-testid={`build-check-viewport-${v.name}`}
    data-ready={v.ready}
  >
    <div className='flex items-baseline justify-between gap-2'>
      <span className='font-medium'>{viewportName(v)}</span>
      <span className={cn("font-medium", v.ready ? ADMIN_TONE.ok.text : ADMIN_TONE.danger.text)}>
        {readyText(v)}
      </span>
    </div>
    {v.build_id && (
      <span className='truncate text-muted-foreground'>
        served <span className='font-mono'>{v.build_id}</span>
        {v.environment ? ` on ${v.environment}` : ""}
      </span>
    )}
    {v.page_errors.map((e, i) => (
      <span
        // biome-ignore lint/suspicious/noArrayIndexKey: page errors have no stable id
        key={i}
        className={cn("break-words font-mono", ADMIN_TONE.danger.text)}
      >
        {e}
      </span>
    ))}
    {v.failed_requests.map((r, i) => (
      <span
        // biome-ignore lint/suspicious/noArrayIndexKey: a URL can fail twice
        key={i}
        className='flex min-w-0 items-baseline gap-1.5'
        data-testid='build-check-failed-request'
      >
        <span className={cn("shrink-0 font-medium font-mono", ADMIN_TONE.danger.text)}>
          {r.status ?? r.error ?? "failed"}
        </span>
        <span className='min-w-0 truncate font-mono text-muted-foreground' title={r.url}>
          {r.url}
        </span>
      </span>
    ))}
  </div>
);
