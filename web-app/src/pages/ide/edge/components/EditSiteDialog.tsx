import { AlertTriangle, CheckCircle2, Loader2, RefreshCw, RotateCcw, XCircle } from "lucide-react";
import type React from "react";
import { useEffect, useMemo, useState } from "react";
import { toast } from "sonner";
import { DESIGN_REFERENCE } from "@/components/ui/reference/tokens";
import { Badge } from "@/components/ui/shadcn/badge";
import { Button } from "@/components/ui/shadcn/button";
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogTitle
} from "@/components/ui/shadcn/dialog";
import { Input } from "@/components/ui/shadcn/input";
import { Label } from "@/components/ui/shadcn/label";
import { Textarea } from "@/components/ui/shadcn/textarea";
import useBulkRewriteSiteCameraRtsp from "@/hooks/api/cameras/useBulkRewriteSiteCameraRtsp";
import useResyncSitePublicIp from "@/hooks/api/cameras/useResyncSitePublicIp";
import { useUnifiCredentialStatus } from "@/hooks/api/cameras/useUnifiCredentialStatus";
import useUpdateSite from "@/hooks/api/cameras/useUpdateSite";
import { cn } from "@/libs/shadcn/utils";
import type { BulkRewriteItem, BulkRewriteResult, Site } from "@/services/api/cameras";
import useCurrentWorkspace from "@/stores/useCurrentWorkspace";

type Props = {
  open: boolean;
  onOpenChange: (open: boolean) => void;
  site: Site | null;
};

/**
 * Edit an existing site. Two things you can do here today:
 *
 *   1. Set `public_ip` — the customer's WAN-facing IP. Required
 *      before the bulk-rewrite section will run.
 *   2. Paste a list of `Camera Name, rtsps://<lan>:7441/<stream_id>`
 *      pairs to rewrite each camera's `rtsp_url` onto the WAN port.
 *
 * The two sections are deliberately separate buttons because the
 * common flow is "set public_ip first, then save, then paste lines."
 * Combining into one submit would force operators to do both at once
 * or surface a half-saved state if validation fails halfway.
 */
const EditSiteDialog: React.FC<Props> = ({ open, onOpenChange, site }) => {
  const { workspace } = useCurrentWorkspace();
  const updateMutation = useUpdateSite(workspace?.id);
  const rewriteMutation = useBulkRewriteSiteCameraRtsp(workspace?.id);
  const resyncMutation = useResyncSitePublicIp(workspace?.id);
  // Drives the UI choice in the resync flow: one-click against the
  // stored credential when configured, or the legacy inline-paste
  // path when not. Stale-while-revalidate is fine — worst case the
  // operator sees the inline prompt for one extra click after they
  // set the credential.
  const { data: credentialStatus } = useUnifiCredentialStatus(workspace?.id);
  const hasStoredCredential = credentialStatus?.configured === true;

  const [publicIp, setPublicIp] = useState("");
  const [region, setRegion] = useState("");
  const [timezone, setTimezone] = useState("");
  const [pasteText, setPasteText] = useState("");
  const [rewriteResult, setRewriteResult] = useState<BulkRewriteResult | null>(null);
  // UniFi API key for the resync button. Kept in dialog-local state
  // (not persisted) so the operator hands it in each time — matches
  // the import flow's current contract.
  const [unifiApiKey, setUnifiApiKey] = useState("");
  const [resyncKeyVisible, setResyncKeyVisible] = useState(false);

  // Re-hydrate every time the dialog opens against a new site.
  useEffect(() => {
    if (!open || !site) return;
    setPublicIp(site.public_ip ?? "");
    setRegion(site.region ?? "");
    setTimezone(site.timezone);
    setPasteText("");
    setRewriteResult(null);
    setUnifiApiKey("");
    setResyncKeyVisible(false);
  }, [open, site]);

  const onResyncFromUnifi = async () => {
    if (!site) return;
    // Two paths converge here:
    //   (a) workspace has a stored credential → send no api_key, the
    //       backend's resolve_api_key pulls from storage
    //   (b) workspace has no stored credential → operator must
    //       paste the key inline
    const inlineKey = unifiApiKey.trim();
    if (!hasStoredCredential && !inlineKey) {
      toast.error("Paste your UniFi API key first, or save one under Settings");
      return;
    }
    try {
      const result = await resyncMutation.mutateAsync({
        siteId: site.id,
        apiKey: inlineKey || null
      });
      // Reflect the new value in the local form state so the operator
      // doesn't see a stale value sitting in the input next to a
      // toast that just announced a change.
      setPublicIp(result.new_public_ip ?? "");
      setResyncKeyVisible(false);
      setUnifiApiKey("");
      if (result.changed) {
        toast.success(
          `Public IP updated · ${result.old_public_ip ?? "(none)"} → ${result.new_public_ip ?? "(cleared)"}`
        );
      } else {
        toast.info(`Public IP unchanged · still ${result.new_public_ip ?? "(none)"}`);
      }
    } catch (err) {
      toast.error(err instanceof Error ? err.message : "Could not resync from UniFi");
    }
  };

  const onSaveSite = async () => {
    if (!site) return;
    // Only send fields that changed to keep the PATCH body minimal —
    // makes audit logs easier to read later and avoids spurious
    // updated_at bumps when the operator only touched one field.
    const body: {
      public_ip?: string | null;
      region?: string | null;
      timezone?: string;
    } = {};
    const trimmedIp = publicIp.trim();
    if (trimmedIp !== (site.public_ip ?? "")) {
      body.public_ip = trimmedIp === "" ? null : trimmedIp;
    }
    const trimmedRegion = region.trim();
    if (trimmedRegion !== (site.region ?? "")) {
      body.region = trimmedRegion === "" ? null : trimmedRegion;
    }
    const trimmedTz = timezone.trim();
    if (trimmedTz !== site.timezone) {
      body.timezone = trimmedTz;
    }
    if (Object.keys(body).length === 0) {
      toast.info("Nothing to save");
      return;
    }
    try {
      await updateMutation.mutateAsync({ siteId: site.id, body });
      toast.success("Site saved");
    } catch (err) {
      toast.error(err instanceof Error ? err.message : "Could not save site");
    }
  };

  // `lines` derived from the textarea — split on newlines, drop blanks.
  // We don't trim here; the backend echoes each line verbatim in its
  // result so the UI can anchor against the operator's exact paste.
  const lines = useMemo(
    () => pasteText.split(/\r?\n/).filter((l) => l.trim().length > 0),
    [pasteText]
  );

  const onRunRewrite = async () => {
    if (!site) return;
    if (!site.public_ip || site.public_ip.trim() === "") {
      toast.error("Set the site's public IP first, then click Save");
      return;
    }
    if (lines.length === 0) {
      toast.error("Paste at least one line first");
      return;
    }
    try {
      const result = await rewriteMutation.mutateAsync({ siteId: site.id, lines });
      setRewriteResult(result);
      const { rewritten } = result.summary;
      if (rewritten === lines.length) {
        toast.success(`Rewrote ${rewritten} of ${lines.length} cameras`);
      } else {
        toast.warning(`Rewrote ${rewritten} of ${lines.length}; see results below`);
      }
    } catch (err) {
      toast.error(err instanceof Error ? err.message : "Could not run rewrite");
    }
  };

  const publicIpSet = (site?.public_ip ?? "").trim().length > 0;

  return (
    <Dialog open={open} onOpenChange={onOpenChange}>
      <DialogContent className={`${DESIGN_REFERENCE} max-w-2xl`}>
        <DialogHeader>
          <DialogTitle>Edit site {site ? `· ${site.name}` : ""}</DialogTitle>
          <DialogDescription>
            Set the customer's WAN-facing IP for pre-edge-box compliance, then paste each camera's
            LAN URL to rewrite onto the public IP + UniFi WAN port (7447). The edge worker
            reconfigures back to LAN URLs once the box is installed at the site.
          </DialogDescription>
        </DialogHeader>

        <div className='flex max-h-[60vh] flex-col gap-5 overflow-y-auto pr-1'>
          {/* ── Section 1: site fields ─────────────────────────────── */}
          <section className='flex flex-col gap-3 border border-border bg-card p-3'>
            <div className='flex items-center justify-between'>
              <h3 className='font-medium text-sm'>Site settings</h3>
              <Button
                size='sm'
                variant='outline'
                onClick={onSaveSite}
                disabled={updateMutation.isPending || !site}
              >
                {updateMutation.isPending ? (
                  <>
                    <Loader2 className='mr-1.5 h-3.5 w-3.5 animate-spin' /> Saving…
                  </>
                ) : (
                  "Save"
                )}
              </Button>
            </div>

            <div className='flex flex-col gap-1.5'>
              <div className='flex items-center justify-between'>
                <Label htmlFor='site-public-ip'>Public IP</Label>
                {site?.source === "unifi" && (
                  // Resync only makes sense for UniFi-imported sites
                  // (manual sites don't have a controller to query).
                  // Two modes:
                  //   • workspace has a stored credential → button
                  //     fires the resync directly (one click)
                  //   • no stored credential → button toggles the
                  //     inline API-key prompt
                  <Button
                    size='sm'
                    variant='ghost'
                    onClick={
                      hasStoredCredential ? onResyncFromUnifi : () => setResyncKeyVisible((v) => !v)
                    }
                    disabled={resyncMutation.isPending}
                    className='h-6 px-2 text-xs'
                    type='button'
                  >
                    {resyncMutation.isPending ? (
                      <>
                        <Loader2 className='mr-1 h-3 w-3 animate-spin' />
                        Resyncing…
                      </>
                    ) : (
                      <>
                        <RefreshCw className='mr-1 h-3 w-3' />
                        Resync from UniFi
                      </>
                    )}
                  </Button>
                )}
              </div>
              <Input
                id='site-public-ip'
                value={publicIp}
                onChange={(e) => setPublicIp(e.target.value)}
                placeholder='50.215.9.17'
                autoComplete='off'
                spellCheck={false}
                className='font-mono'
              />
              <span className='text-muted-foreground text-xs'>
                {site?.source === "unifi"
                  ? "Pre-filled from the UniFi controller on import. Click Resync if the customer's ISP rotated the IP. Manual edits also work — click Save to commit."
                  : "Customer's WAN-facing IP. Required for the bulk rewrite below — set this and click Save first. Clear to disable."}
              </span>
              {resyncKeyVisible && !hasStoredCredential && (
                <div className='mt-1 flex flex-col gap-1.5 border border-border bg-muted/30 p-2'>
                  <Label htmlFor='site-resync-key' className='text-xs'>
                    UniFi API key
                  </Label>
                  <Input
                    id='site-resync-key'
                    type='password'
                    value={unifiApiKey}
                    onChange={(e) => setUnifiApiKey(e.target.value)}
                    placeholder='ui_xxx…'
                    autoComplete='off'
                    spellCheck={false}
                    className='h-8 font-mono text-xs'
                  />
                  <div className='flex items-center gap-2'>
                    <Button
                      size='sm'
                      onClick={onResyncFromUnifi}
                      disabled={resyncMutation.isPending || unifiApiKey.trim() === ""}
                    >
                      {resyncMutation.isPending ? (
                        <>
                          <Loader2 className='mr-1.5 h-3 w-3 animate-spin' />
                          Resyncing…
                        </>
                      ) : (
                        "Fetch current IP"
                      )}
                    </Button>
                    <Button
                      size='sm'
                      variant='ghost'
                      onClick={() => {
                        setResyncKeyVisible(false);
                        setUnifiApiKey("");
                      }}
                      disabled={resyncMutation.isPending}
                    >
                      Cancel
                    </Button>
                    <span className='text-muted-foreground text-xs'>
                      One-off. Save the key under Devices → UniFi credentials to skip this prompt
                      every time.
                    </span>
                  </div>
                </div>
              )}
            </div>

            <div className='grid grid-cols-2 gap-3'>
              <div className='flex flex-col gap-1.5'>
                <Label htmlFor='site-timezone'>Timezone</Label>
                <Input
                  id='site-timezone'
                  value={timezone}
                  onChange={(e) => setTimezone(e.target.value)}
                  placeholder='America/Los_Angeles'
                  autoComplete='off'
                  spellCheck={false}
                  className='font-mono'
                />
              </div>
              <div className='flex flex-col gap-1.5'>
                <Label htmlFor='site-region'>Region</Label>
                <Input
                  id='site-region'
                  value={region}
                  onChange={(e) => setRegion(e.target.value)}
                  placeholder='us-west'
                  autoComplete='off'
                  spellCheck={false}
                />
              </div>
            </div>
          </section>

          {/* ── Section 2: bulk rewrite ────────────────────────────── */}
          <section className='flex flex-col gap-3 border border-border bg-card p-3'>
            <div className='flex items-center justify-between'>
              <h3 className='font-medium text-sm'>Rewrite camera RTSPs from LAN paste</h3>
              <Button
                size='sm'
                onClick={onRunRewrite}
                disabled={rewriteMutation.isPending || !publicIpSet || lines.length === 0}
              >
                {rewriteMutation.isPending ? (
                  <>
                    <Loader2 className='mr-1.5 h-3.5 w-3.5 animate-spin' /> Rewriting…
                  </>
                ) : (
                  <>
                    <RotateCcw className='mr-1.5 h-3.5 w-3.5' /> Rewrite {lines.length || ""}
                  </>
                )}
              </Button>
            </div>

            {!publicIpSet && (
              <div className='flex items-start gap-2 border border-dot-warning/40 bg-dot-warning/5 p-2 text-status-warning text-xs'>
                <AlertTriangle className='h-3.5 w-3.5 shrink-0' />
                <span>
                  Set the site's public IP above and click Save before pasting URLs — the rewrite
                  can't run without it.
                </span>
              </div>
            )}

            <div className='flex flex-col gap-1.5'>
              <Label htmlFor='site-paste'>Paste one camera per line</Label>
              <Textarea
                id='site-paste'
                value={pasteText}
                onChange={(e) => setPasteText(e.target.value)}
                placeholder={
                  "Camera 1, rtsps://192.168.0.1:7441/abc123\nCamera 2, rtsps://192.168.0.1:7441/def456"
                }
                spellCheck={false}
                className='min-h-32 font-mono text-xs'
              />
              <span className='text-muted-foreground text-xs'>
                Format:{" "}
                <span className='font-mono'>
                  Camera Name, rtsps://&lt;lan-ip&gt;:7441/&lt;stream_id&gt;
                </span>
                . The system extracts the stream ID and builds{" "}
                <span className='font-mono'>
                  rtsp://{publicIp || "<public_ip>"}:7447/&lt;stream_id&gt;
                </span>
                .
              </span>
            </div>

            {rewriteResult && <RewriteResultTable result={rewriteResult} />}
          </section>
        </div>

        <DialogFooter>
          <Button variant='outline' onClick={() => onOpenChange(false)}>
            Close
          </Button>
        </DialogFooter>
      </DialogContent>
    </Dialog>
  );
};

const RewriteResultTable: React.FC<{ result: BulkRewriteResult }> = ({ result }) => {
  const { items, summary } = result;
  return (
    <div className='flex flex-col gap-2'>
      <div className='flex flex-wrap gap-1.5 text-xs'>
        <Badge variant='outline' className='border-dot-success/40 bg-dot-success/5'>
          <CheckCircle2 className='mr-1 h-3 w-3 text-status-success' /> {summary.rewritten}{" "}
          rewritten
        </Badge>
        {summary.no_match > 0 && (
          <Badge variant='outline' className='border-dot-warning/40 bg-dot-warning/5'>
            {summary.no_match} no match
          </Badge>
        )}
        {summary.invalid_url > 0 && (
          <Badge variant='outline' className='border-destructive/40 bg-destructive/5'>
            {summary.invalid_url} invalid URL
          </Badge>
        )}
        {summary.parse_error > 0 && (
          <Badge variant='outline' className='border-destructive/40 bg-destructive/5'>
            {summary.parse_error} parse error
          </Badge>
        )}
        {summary.ambiguous_name > 0 && (
          <Badge variant='outline' className='border-destructive/40 bg-destructive/5'>
            {summary.ambiguous_name} ambiguous
          </Badge>
        )}
      </div>
      <ul className='flex flex-col gap-1 text-xs'>
        {items.map((item, idx) => (
          <li
            // biome-ignore lint/suspicious/noArrayIndexKey: result list is read-only — replaced atomically on each rewrite — and items never reorder or splice. line+idx keeps duplicates distinct (operator may intentionally paste the same line twice to retry).
            key={`${idx}-${item.line}`}
            className={cn(
              "flex items-start gap-2 border px-2 py-1.5",
              item.outcome === "rewritten"
                ? "border-dot-success/30 bg-dot-success/5"
                : "border-destructive/30 bg-destructive/5"
            )}
          >
            <ResultIcon item={item} />
            <div className='flex min-w-0 flex-1 flex-col gap-0.5'>
              <span className='truncate font-mono text-[11px] text-muted-foreground'>
                {item.line}
              </span>
              {item.outcome === "rewritten" ? (
                <span className='truncate font-mono text-[11px]'>→ {item.new_url}</span>
              ) : (
                <span className='text-[11px] text-destructive'>{item.error}</span>
              )}
            </div>
          </li>
        ))}
      </ul>
    </div>
  );
};

const ResultIcon: React.FC<{ item: BulkRewriteItem }> = ({ item }) => {
  if (item.outcome === "rewritten") {
    return <CheckCircle2 className='mt-0.5 h-3.5 w-3.5 shrink-0 text-status-success' />;
  }
  if (item.outcome === "no_match") {
    return <AlertTriangle className='mt-0.5 h-3.5 w-3.5 shrink-0 text-status-warning' />;
  }
  return <XCircle className='mt-0.5 h-3.5 w-3.5 shrink-0 text-destructive' />;
};

export default EditSiteDialog;
