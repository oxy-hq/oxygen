import { ChevronRight, ExternalLink } from "lucide-react";
import { Button } from "@/components/ui/shadcn/button";
import {
  Collapsible,
  CollapsibleContent,
  CollapsibleTrigger
} from "@/components/ui/shadcn/collapsible";
import { Skeleton } from "@/components/ui/shadcn/skeleton";
import { useAppDebug } from "@/hooks/api/customApps/useCustomApps";
import { cn } from "@/libs/shadcn/utils";
import { resolveBundleUrl } from "@/pages/admin/AdminCustomApps/resolveBundleUrl";
import { AdminAsync } from "@/pages/admin/components/AdminAsync";
import type { CustomApp } from "@/types/apps";
import { CopyButton } from "../../../AppsTable/components/UrlActions";

/**
 * Diagnostics dossier for the selected app: what oxy currently resolves (the
 * channel's build, manifest source, raw manifest) as a scannable health readout
 * rather than a wall of pills. Two health chips up top answer "is this app wired
 * right?" at a glance; URLs + identity are compact rows; the raw manifest is a
 * copyable, collapsed-by-default block so it stops dominating the panel.
 *
 * Read-only by design — Settings owns mutations. Project/branch come from the
 * admin row (`app`), not the bundle-public debug snapshot.
 */
export const AppInfo = ({ app }: { app: CustomApp }) => {
  const debug = useAppDebug(app.org_slug, app.slug);

  return (
    <AdminAsync
      query={debug}
      noun='the diagnostic snapshot'
      className='p-4'
      // Two blocks, not bars: the readout is a health row above a manifest panel,
      // and equal-height bars would misstate what is coming.
      skeleton={
        <div className='space-y-4'>
          <Skeleton className='h-20 w-full' />
          <Skeleton className='h-40 w-full' />
        </div>
      }
    >
      {(data) => {
        const manifestOk = !!data.manifest && !data.manifest_error;
        // The bundle is the build the resolved channel points at. There is no
        // directory to look for: every app serves from the build store.
        const bundleOk = data.build !== null;
        return (
          <div className='space-y-4 p-4 pt-0'>
            {/* Health readout — the two things that can actually be broken. */}
            <div className='grid grid-cols-2 gap-2'>
              <HealthChip
                label='Bundle'
                ok={bundleOk}
                // The channel it read, or `missing` — short enough not to truncate in
                // the narrow details column.
                value={bundleOk ? data.channel : "missing"}
              />
              <HealthChip
                label='Manifest'
                ok={manifestOk}
                value={
                  !manifestOk
                    ? "error"
                    : data.manifest_source === "db_override"
                      ? "DB override"
                      : "bundled"
                }
              />
            </div>

            {/* URLs — what to share with the customer or use in iframes. */}
            <Section title='URLs'>
              <UrlRow label='Subpath' url={app.url} />
              {app.url_subdomain && (
                <UrlRow label='Subdomain' url={app.url_subdomain} recommended absolute />
              )}
            </Section>

            {/* Identity + what oxy resolved. Ids are shortened, not wrapped: nobody
          reads a UUID, they copy it, and two full ids wrapping across four lines
          were most of this block's height. */}
            <Section title='Identity'>
              <KVId k='App ID' id={data.app.id} />
              <KVId k='Workspace' id={app.project_id} />
              <KV k='Branch' v={app.branch} mono />
              <KV k='Status' v={data.app.status} />
              {/* Only a row left over from a removed source kind says anything here —
            and what it says is why the app fails. */}
              {data.app.source_type !== "s3" && (
                <KV k='Source' v={`${data.app.source_type} (removed)`} />
              )}
            </Section>

            {data.manifest_error && (
              <Section title='Manifest error' tone='destructive'>
                <pre className='whitespace-pre-wrap rounded-md bg-destructive/10 p-3 text-destructive text-xs'>
                  {data.manifest_error}
                </pre>
              </Section>
            )}

            {/* Raw manifest — collapsed by default so it stops dominating; copyable. */}
            {manifestOk && <ManifestBlock manifest={data.manifest} />}
          </div>
        );
      }}
    </AdminAsync>
  );
};

/**
 * A status chip for one integrity check. Only a broken reading carries colour: a
 * healthy chip's dot is neutral, the same rule the app list follows — colour on
 * these pages means something needs a look. One line, label and reading sharing
 * the row, so two chips fit a narrow column without either one truncating.
 */
const HealthChip = ({ label, ok, value }: { label: string; ok: boolean; value: string }) => (
  <div
    className='flex min-w-0 items-center gap-1.5 rounded-md border bg-card px-2.5 py-1.5 text-xs'
    data-testid={`admin-app-info-check-${label.toLowerCase()}`}
  >
    <span
      aria-hidden
      className={cn(
        "size-1.5 shrink-0 rounded-full",
        ok ? "bg-muted-foreground/40" : "bg-destructive"
      )}
    />
    <span className='shrink-0 text-muted-foreground'>{label}</span>
    <span
      className={cn("min-w-0 flex-1 truncate text-right font-medium", !ok && "text-destructive")}
      title={value}
    >
      {value}
    </span>
  </div>
);

const ManifestBlock = ({ manifest }: { manifest: unknown }) => {
  const json = JSON.stringify(manifest, null, 2);
  return (
    <Section title='Manifest'>
      <Collapsible>
        <div className='flex items-center justify-between'>
          <CollapsibleTrigger className='group flex items-center gap-1 rounded text-muted-foreground text-xs hover:text-foreground'>
            <ChevronRight className='size-3 transition-transform group-data-[state=open]:rotate-90' />
            View raw manifest
          </CollapsibleTrigger>
          <CopyButton value={json} label='manifest' />
        </div>
        <CollapsibleContent>
          <pre className='mt-2 max-h-80 overflow-auto whitespace-pre-wrap rounded-md border bg-muted/40 p-3 font-mono text-[11px] text-foreground/90'>
            {json}
          </pre>
        </CollapsibleContent>
      </Collapsible>
    </Section>
  );
};

const Section = ({
  title,
  children,
  tone
}: {
  title: string;
  children: React.ReactNode;
  tone?: "destructive";
}) => (
  <div>
    {/* Sentence case, not tracked caps: these are sub-headings inside a
        dossier section, and the admin convention reserves caps for the
        collapsible section label above them. Hierarchy comes from weight. */}
    <h3
      className={cn(
        "mb-1.5 font-medium text-xs",
        tone === "destructive" ? "text-destructive" : "text-foreground"
      )}
    >
      {title}
    </h3>
    <div className='space-y-1.5'>{children}</div>
  </div>
);

/**
 * A label/value row. The label holds a fixed narrow column and the value starts
 * right after it, wrapping — rather than the two being pushed to opposite edges
 * with a lake of dead space between them and the value truncated anyway. That
 * pairing is what made this panel demand width it never used.
 */
const KV = ({ k, v, mono }: { k: string; v: string; mono?: boolean }) => (
  <div className='flex items-baseline gap-3 border-b py-1 text-xs last:border-0'>
    <span className='w-20 shrink-0 text-muted-foreground text-xs'>{k}</span>
    <span className={cn("min-w-0 flex-1 break-all", mono && "font-mono text-xs")}>{v}</span>
  </div>
);

/**
 * An id row: the first eight characters, the rest behind a copy button. The full
 * value is the tooltip, so it is still one hover away when someone does need to
 * read it.
 */
const KVId = ({ k, id }: { k: string; id: string }) => (
  <div className='flex items-center gap-3 border-b py-1 text-xs last:border-0'>
    <span className='w-20 shrink-0 text-muted-foreground'>{k}</span>
    <span className='min-w-0 flex-1 font-mono' title={id}>
      {id.slice(0, 8)}
    </span>
    <CopyButton value={id} label={k} />
  </div>
);

const UrlRow = ({
  label,
  url,
  recommended,
  absolute
}: {
  label: string;
  url: string;
  recommended?: boolean;
  absolute?: boolean;
}) => {
  const copyValue = absolute ? url : new URL(url, window.location.origin).toString();
  const openHref = absolute ? url : resolveBundleUrl(url);
  return (
    <div className='flex items-center gap-1.5'>
      <div className='min-w-0 flex-1 overflow-hidden'>
        <div className='flex items-center gap-1.5'>
          <span className='text-muted-foreground text-xs'>{label}</span>
          {recommended && <span className='text-muted-foreground text-xs'>(recommended)</span>}
        </div>
        <div className='truncate font-mono text-xs' title={url}>
          {url}
        </div>
      </div>
      <CopyButton value={copyValue} label={label} />
      <Button
        variant='ghost'
        size='icon'
        className='size-6 shrink-0'
        onClick={() => window.open(openHref, "_blank", "noopener,noreferrer")}
        aria-label={`Open ${label} in a new tab`}
      >
        <ExternalLink className='size-3.5' />
      </Button>
    </div>
  );
};
