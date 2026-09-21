import { Globe, Loader2 } from "lucide-react";
import { Label } from "@/components/ui/shadcn/label";
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue
} from "@/components/ui/shadcn/select";
import { Switch } from "@/components/ui/shadcn/switch";
import {
  useAdminOrgSubdomain,
  useSetAdminOrgSubdomain
} from "@/hooks/api/adminTenants/useAdminOrgSubdomain";
import { AdminAsync } from "../../components/AdminAsync";
import { AdminStatusPill } from "../../components/AdminStatusPill";

// Radix Select can't carry an empty-string value; sentinel maps back to null.
const NO_DEFAULT = "__none__";

/**
 * Oxy-staff control for an org's opt-in bare subdomain (`<org-slug>.<zone>`).
 * Lives in the org detail Settings tab. The subdomain label is the org slug
 * (not editable); staff toggle it on/off and choose the default project.
 * Customers can't toggle this — they see read-only status in their own
 * settings. See internal-docs/org-subdomain-infra.md.
 */
export function OrgSubdomainSettings({ orgId }: { orgId: string }) {
  const subdomain = useAdminOrgSubdomain(orgId);
  const data = subdomain.data;
  const setSub = useSetAdminOrgSubdomain();

  // The controls below all read `data`, so the gate is the whole section. It used to be
  // `isLoading || !data` around a spinner, which spun forever on a failed fetch.
  // `AdminAsync` separates the two and offers a Retry; the guard covers every
  // non-success state, so its render callback is unreachable.
  if (!data) {
    return (
      <AdminAsync query={subdomain} noun='the subdomain settings' rows={2}>
        {() => null}
      </AdminAsync>
    );
  }

  const update = (next: { enabled?: boolean; default_workspace_id?: string | null }) =>
    setSub.mutate({
      orgId,
      body: {
        enabled: next.enabled ?? data.enabled,
        default_workspace_id:
          next.default_workspace_id !== undefined
            ? next.default_workspace_id
            : data.default_workspace_id
      }
    });

  return (
    <section className='space-y-4 rounded-lg border border-border/60 bg-card p-6'>
      <div className='space-y-1'>
        <div className='flex items-center gap-2'>
          <Globe className='size-4 text-muted-foreground' />
          <h3 className='font-semibold text-sm'>Org subdomain</h3>
          {data.enabled ? <AdminStatusPill tone='ok' label='Enabled' /> : null}
        </div>
        <p className='text-muted-foreground text-xs'>
          Serve this org at <code className='font-mono'>{data.subdomain}.&lt;zone&gt;</code> — a
          branded entry scoped to the default project, with the org's custom apps at{" "}
          <code className='font-mono'>/a/&lt;slug&gt;/</code>. Oxy-staff only; the label is the org
          slug (not editable).
        </p>
      </div>

      {data.reserved && (
        <p className='text-destructive text-xs'>
          The slug “{data.subdomain}” is a reserved infra label — it can't be used as a subdomain.
        </p>
      )}

      <div className='flex items-center justify-between gap-4'>
        <div className='flex flex-col'>
          <span className='font-medium text-xs'>Enabled</span>
          <span className='text-muted-foreground text-xs'>
            {data.enabled && data.url ? (
              <>
                Live at <span className='font-mono'>{data.url}</span>
              </>
            ) : (
              "Off — no public subdomain"
            )}
          </span>
        </div>
        <div className='flex items-center gap-3'>
          {setSub.isPending && <Loader2 className='size-3.5 animate-spin text-muted-foreground' />}
          <Switch
            checked={data.enabled}
            disabled={setSub.isPending || (data.reserved && !data.enabled)}
            onCheckedChange={(next) => update({ enabled: next })}
            aria-label='Toggle org subdomain'
          />
        </div>
      </div>

      <div className='space-y-1.5'>
        <Label htmlFor='org-subdomain-default'>Default project</Label>
        <Select
          value={data.default_workspace_id ?? NO_DEFAULT}
          disabled={setSub.isPending}
          onValueChange={(v) => update({ default_workspace_id: v === NO_DEFAULT ? null : v })}
        >
          <SelectTrigger id='org-subdomain-default'>
            <SelectValue placeholder='Pick a project' />
          </SelectTrigger>
          <SelectContent>
            <SelectItem value={NO_DEFAULT}>No default (dispatcher picks)</SelectItem>
            {data.workspaces.map((w) => (
              <SelectItem key={w.id} value={w.id}>
                {w.name}
              </SelectItem>
            ))}
          </SelectContent>
        </Select>
      </div>
    </section>
  );
}
