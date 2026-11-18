import type { Release, ReleaseScreenVitals, ReleaseTimeline } from "@/types/appReleases";
import { clsText, deltaText, msText, rateText } from "../lanes";

/**
 * What one release measured, beside the release it is compared with: function
 * calls, failure rate and route p95, then Web Vitals per screen. A part the server
 * could not read says so — it never renders as zeros.
 */
export const ReleaseMetrics = ({
  release,
  previous,
  parts
}: {
  release: Release;
  previous: Release | null;
  parts: ReleaseTimeline["parts"];
}) => (
  <div className='flex flex-col gap-2' data-testid={`admin-app-release-${release.index}-metrics`}>
    <CallStrip release={release} previous={previous} parts={parts} />
    <ScreenTable release={release} previous={previous} parts={parts} />
  </div>
);

const Stat = ({ label, value, delta }: { label: string; value: string; delta?: string | null }) => (
  <div className='flex min-w-0 flex-col'>
    <span className='text-[10px] text-muted-foreground uppercase tracking-[0.16em]'>{label}</span>
    <span className='text-sm tabular-nums'>{value}</span>
    {delta && <span className='text-muted-foreground text-xs tabular-nums'>{delta}</span>}
  </div>
);

const CallStrip = ({
  release,
  previous,
  parts
}: {
  release: Release;
  previous: Release | null;
  parts: ReleaseTimeline["parts"];
}) => {
  const m = release.metrics;
  if (!m) {
    const why = release.pruned
      ? "Build pruned — its calls were deleted with it."
      : release.unpublished
        ? "Nothing was served."
        : parts.metrics === "unavailable"
          ? "Couldn't measure this release's function calls."
          : "No calls measured.";
    return <p className='text-muted-foreground text-xs'>{why}</p>;
  }
  const p = previous?.metrics ?? null;
  const vs = (d: string | null) => (d && previous ? `${d} vs before` : null);
  return (
    <div className='grid @md:grid-cols-4 grid-cols-2 gap-x-4 gap-y-2'>
      <Stat label='Calls' value={m.calls.toLocaleString("en")} />
      <Stat
        label='Failure rate'
        value={`${rateText(m.failure_rate_bp)} (${m.failed})`}
        delta={vs(deltaText(p?.failure_rate_bp ?? null, m.failure_rate_bp, "bp"))}
      />
      <Stat
        label='Route p95'
        value={msText(m.p95_ms)}
        delta={vs(deltaText(p?.p95_ms ?? null, m.p95_ms, "ms"))}
      />
      {m.shed > 0 && <Stat label='Shed' value={m.shed.toLocaleString("en")} />}
    </div>
  );
};

const ScreenTable = ({
  release,
  previous,
  parts
}: {
  release: Release;
  previous: Release | null;
  parts: ReleaseTimeline["parts"];
}) => {
  if (parts.vitals === "unavailable") {
    return <p className='text-muted-foreground text-xs'>Couldn't read Web Vitals.</p>;
  }
  if (release.vitals.length === 0) {
    return <p className='text-muted-foreground text-xs'>No Web Vitals recorded.</p>;
  }
  const before = (path: string) => previous?.vitals.find((v) => v.path === path) ?? null;
  return (
    <div
      className='grid grid-cols-[minmax(0,1fr)_auto_auto_auto_auto] gap-x-3 text-xs'
      data-testid={`admin-app-release-${release.index}-vitals`}
    >
      <div className='col-span-full grid grid-cols-subgrid border-border/60 border-b pb-1 text-[10px] text-muted-foreground uppercase tracking-[0.16em]'>
        <span>Screen</span>
        <span className='text-right'>Loads</span>
        <span className='text-right'>LCP p75</span>
        <span className='text-right' title='Worst interaction in the first 10 s, p75'>
          INP*
        </span>
        <span className='text-right'>CLS p75</span>
      </div>
      {release.vitals.map((v) => (
        <ScreenRow key={v.path} v={v} was={before(v.path)} />
      ))}
      <p className='col-span-full pt-1 text-[10px] text-muted-foreground'>
        * INP and CLS cover a page's first 10 s.
        {release.more_screens ? " More screens not shown." : ""}
      </p>
    </div>
  );
};

const ScreenRow = ({ v, was }: { v: ReleaseScreenVitals; was: ReleaseScreenVitals | null }) => {
  const cell = (value: string, delta: string | null) => (
    <span className='text-right tabular-nums' title={delta ? `${delta} vs before` : undefined}>
      {value}
      {delta && <span className='block text-[10px] text-muted-foreground'>{delta}</span>}
    </span>
  );
  return (
    <div
      className='col-span-full grid grid-cols-subgrid py-1'
      data-testid={`admin-app-release-screen-${v.path}`}
    >
      <span className='truncate font-mono' title={v.path}>
        {v.path}
      </span>
      <span className='text-right tabular-nums'>{v.samples}</span>
      {cell(msText(v.lcp_p75), deltaText(was?.lcp_p75 ?? null, v.lcp_p75, "ms"))}
      {cell(msText(v.inp_p75), deltaText(was?.inp_p75 ?? null, v.inp_p75, "ms"))}
      {cell(clsText(v.cls_p75), deltaText(was?.cls_p75 ?? null, v.cls_p75, "cls_milli"))}
    </div>
  );
};
