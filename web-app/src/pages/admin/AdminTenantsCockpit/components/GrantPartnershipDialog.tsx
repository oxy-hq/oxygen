import { useQueryClient } from "@tanstack/react-query";
import { isAxiosError } from "axios";
import { useState } from "react";
import { toast } from "sonner";
import { DESIGN_REFERENCE } from "@/components/ui/reference/tokens";
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
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue
} from "@/components/ui/shadcn/select";
import { Switch } from "@/components/ui/shadcn/switch";
import { useAdminPartners } from "@/hooks/api/adminPartners";
import queryKeys from "@/hooks/api/queryKey";
import useCurrentUser from "@/hooks/api/users/useCurrentUser";
import { cn } from "@/libs/shadcn/utils";
import { AdminPartnersService } from "@/services/api/adminPartners";
import type { AdminOrgMeta } from "@/services/api/adminTenants";
import {
  type AdminPartnerCapabilities,
  CAPABILITY_LABELS,
  DEFAULT_PARTNER_CEILING,
  OWNER_ONLY_CAPABILITIES
} from "@/types/adminPartners";
import OrgPicker from "./OrgPicker";

const CAPS = Object.keys(CAPABILITY_LABELS) as (keyof AdminPartnerCapabilities)[];

/**
 * Put `org` under a partner's administration.
 *
 * A partner is **an org that holds a grant** — there is no separate partner
 * entity to name. So the choice here is *which organization* administers this
 * one: an org that is already a partner, or one we promote now (setting its
 * **ceiling** — the maximum it can ever do — at the same time).
 *
 * One transactional call: grant + ceiling + client attachment + first partner
 * admin. A mid-flight failure rolls the whole thing back rather than leaving a
 * half-provisioned partner behind.
 */
export default function GrantPartnershipDialog({
  open,
  onOpenChange,
  org,
  suggestedAdminEmail
}: {
  open: boolean;
  onOpenChange: (o: boolean) => void;
  /** The CLIENT — the org being handed over. */
  org: { id: string; name: string };
  suggestedAdminEmail?: string;
}) {
  const qc = useQueryClient();
  const { data: partners } = useAdminPartners();
  const { data: me } = useCurrentUser();
  const isOwner = !!me?.is_owner;

  const [mode, setMode] = useState<"existing" | "promote">("existing");
  const [existingId, setExistingId] = useState<string>("");
  const [promoted, setPromoted] = useState<AdminOrgMeta | null>(null);
  const [email, setEmail] = useState(suggestedAdminEmail ?? "");
  const [caps, setCaps] = useState<AdminPartnerCapabilities>(DEFAULT_PARTNER_CEILING);
  const [busy, setBusy] = useState(false);

  const partnerOrgId = mode === "existing" ? existingId : (promoted?.id ?? "");
  const canSubmit = !busy && !!partnerOrgId;

  async function submit() {
    setBusy(true);
    try {
      await AdminPartnersService.grant({
        partner_org_id: partnerOrgId,
        first_client_org_id: org.id,
        // The ceiling is only meaningful when we're promoting an org now — an
        // existing partner already has one, and the server ignores it.
        ...(mode === "promote" ? { capabilities: caps } : {}),
        ...(email.trim() ? { partner_admin_email: email.trim() } : {})
      });
      qc.invalidateQueries({ queryKey: queryKeys.adminPartner.all });
      qc.invalidateQueries({ queryKey: queryKeys.adminOrgs.all });
      toast.success(`${org.name} is now managed by a partner`);
      onOpenChange(false);
    } catch (err) {
      const status = isAxiosError(err) ? err.response?.status : undefined;
      toast.error(
        status === 409
          ? `${org.name} is already managed by another partner.`
          : status === 403
            ? "Billing and Secrets can only be granted by a Global Owner."
            : status === 400
              ? "The partner admin must already be a member of the partner org."
              : "Failed to grant the partnership. Nothing was changed."
      );
    } finally {
      setBusy(false);
    }
  }

  return (
    <Dialog open={open} onOpenChange={onOpenChange}>
      <DialogContent className={`${DESIGN_REFERENCE} max-w-md`}>
        <DialogHeader>
          <DialogTitle>Grant partnership</DialogTitle>
          <DialogDescription>
            Choose the organization that will administer <b>{org.name}</b>.
          </DialogDescription>
        </DialogHeader>

        <div className='space-y-4'>
          <div className='flex gap-1 bg-muted p-1'>
            {(["existing", "promote"] as const).map((m) => (
              <button
                key={m}
                type='button'
                onClick={() => setMode(m)}
                className={cn(
                  "flex-1 px-2 py-1 font-medium text-xs",
                  mode === m ? "bg-background" : "text-muted-foreground"
                )}
              >
                {m === "existing" ? "Existing partner" : "Promote an org"}
              </button>
            ))}
          </div>

          {mode === "existing" ? (
            <div className='space-y-1'>
              <Label>Partner</Label>
              <Select value={existingId} onValueChange={setExistingId}>
                <SelectTrigger>
                  <SelectValue placeholder='Choose a partner…' />
                </SelectTrigger>
                <SelectContent className={DESIGN_REFERENCE}>
                  {(partners ?? []).map((p) => (
                    <SelectItem key={p.org_id} value={p.org_id}>
                      {p.name}
                    </SelectItem>
                  ))}
                </SelectContent>
              </Select>
            </div>
          ) : (
            <>
              <div className='space-y-1'>
                <Label>Organization to promote</Label>
                <OrgPicker
                  label={promoted ? promoted.name : "Select organization…"}
                  exclude={[org.id]}
                  onPick={setPromoted}
                />
                <p className='text-muted-foreground text-xs'>
                  It stays a normal organization — it just gains the right to administer others.
                </p>
              </div>
              <div className='space-y-1.5'>
                <Label>Ceiling</Label>
                <p className='text-muted-foreground text-xs'>
                  The maximum this partner can ever do. Its own admin hands out roles inside this —
                  never beyond it.
                </p>
                <div className='flex flex-wrap gap-x-3 gap-y-1.5'>
                  {CAPS.map((k) => {
                    const ownerOnly = OWNER_ONLY_CAPABILITIES.includes(k);
                    const locked = ownerOnly && !isOwner;
                    return (
                      <label
                        key={k}
                        htmlFor={`grant-cap-${k}`}
                        className={cn("flex items-center gap-1.5 text-xs", locked && "opacity-50")}
                        title={locked ? "Global Owner only" : undefined}
                      >
                        <Switch
                          id={`grant-cap-${k}`}
                          checked={caps[k]}
                          disabled={locked}
                          onCheckedChange={(v) => setCaps((c) => ({ ...c, [k]: v }))}
                        />
                        {CAPABILITY_LABELS[k]}
                      </label>
                    );
                  })}
                </div>
              </div>
            </>
          )}

          <div className='space-y-1'>
            <Label>First partner admin (optional)</Label>
            <Input
              type='email'
              placeholder='admin@partner.com'
              value={email}
              onChange={(e) => setEmail(e.target.value)}
            />
            <p className='text-muted-foreground text-xs'>
              Must already be a member of the partner org. They staff the rest of the partner
              themselves.
            </p>
          </div>
        </div>

        <DialogFooter>
          <Button variant='ghost' onClick={() => onOpenChange(false)}>
            Cancel
          </Button>
          <Button disabled={!canSubmit} onClick={submit}>
            Grant partnership
          </Button>
        </DialogFooter>
      </DialogContent>
    </Dialog>
  );
}
