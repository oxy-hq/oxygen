import { Layers } from "lucide-react";
import { cn } from "@/libs/shadcn/utils";
import { ADMIN_TONE } from "@/pages/admin/components/adminTone";
import type { AppsRollup, OrgRow } from "../../appBrowserModel";
import { STATUS_TONE } from "../../appStatus";
import { ColumnPane } from "./ColumnPane";
import type { SelectHow } from "./useColumnKeys";

/** The row that stands for every app on the deployment, whatever org it is under. */
export const ALL_ORGS = "__all__";

/**
 * The first tier: an `All apps` row, then every org that has an app.
 *
 * **One trailing signal per row, and it is the one worth reading.** When an org has apps
 * that need a person, the row says how many, toned by the worst of them — danger for a
 * `down`, warning for a `degraded` or an unmeasured one. When it does not, the row says
 * how many apps it has, quietly. A status dot *and* a count would make every row carry
 * two numbers and one of them would always be noise; the exact per-app status is one
 * tier to the right, where it belongs.
 */
export const OrgsColumn = ({
  orgs,
  all,
  selectedId,
  onSelect,
  onLeaveColumn
}: {
  orgs: readonly OrgRow[];
  /** The same fact over the whole deployment, for the `All apps` row. */
  all: AppsRollup;
  selectedId: string;
  onSelect: (id: string, how: SelectHow) => void;
  onLeaveColumn: (delta: -1 | 1) => void;
}) => (
  <ColumnPane
    id='orgs'
    label='Organizations'
    depth={0}
    width='w-52 2xl:w-64'
    selectedId={selectedId}
    empty='No org has a custom app yet.'
    onSelect={onSelect}
    onLeaveColumn={onLeaveColumn}
    rows={[
      {
        id: ALL_ORGS,
        label: "All apps",
        href: "/admin/apps",
        children: (
          <>
            <Layers className='size-3.5 shrink-0' aria-hidden />
            <span className='min-w-0 flex-1 truncate'>All apps</span>
            <Trailing rollup={all} />
          </>
        )
      },
      ...orgs.map((org) => ({
        id: org.orgSlug,
        label: org.orgSlug,
        href: `/admin/apps?org=${encodeURIComponent(org.orgSlug)}`,
        children: (
          <>
            {/* An empty glyph slot, not an icon. Every org would carry the same building
                glyph, so it would encode nothing and only compete with the slug; leaving
                the slot keeps the rows aligned with `All apps`, whose icon is then the
                one thing in this column that means something — that the row is not an
                org. In the other two tiers every glyph differs and so earns its place:
                an app's own mark, a page's own icon. */}
            <span className='size-3.5 shrink-0' aria-hidden />
            {/* A long slug truncates, so the full one has to be reachable without
                opening the org. */}
            <span className='min-w-0 flex-1 truncate font-mono' title={org.orgSlug}>
              {org.orgSlug}
            </span>
            <Trailing rollup={org} />
          </>
        )
      }))
    ]}
  />
);

const Trailing = ({ rollup }: { rollup: AppsRollup }) => {
  if (rollup.attention === 0) {
    return (
      <span className='shrink-0 tabular-nums opacity-60' title={`${rollup.appCount} apps`}>
        {rollup.appCount}
      </span>
    );
  }
  // `attention > 0` means at least one app is down, degraded or unmeasured, so `worst` is
  // one of those three; the fallback only exists so the type does not need a cast.
  const tone = ADMIN_TONE[STATUS_TONE[rollup.worst ?? "degraded"]];
  return (
    <span
      className={cn("shrink-0 px-1 py-px font-medium tabular-nums", tone.bg, tone.text)}
      title={`${rollup.attention} of ${rollup.appCount} apps need attention`}
    >
      {rollup.attention}
    </span>
  );
};
