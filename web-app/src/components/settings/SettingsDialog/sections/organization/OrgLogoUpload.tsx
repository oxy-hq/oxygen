import { ImageUp, Trash2 } from "lucide-react";
import { useRef, useState } from "react";
import { toast } from "sonner";
import { workspaceLogoUrl } from "@/components/Shell/logoUrl";
import { Button } from "@/components/ui/shadcn/button";
import { Label } from "@/components/ui/shadcn/label";
import { useDeleteOrgLogo, useUploadOrgLogo } from "@/hooks/api/organizations";
import { OrganizationService } from "@/services/api/organizations";
import useCurrentOrg from "@/stores/useCurrentOrg";
import useCurrentWorkspace from "@/stores/useCurrentWorkspace";
import type { Organization } from "@/types/organization";

const ACCEPT = "image/png,image/jpeg,image/svg+xml,image/webp,image/gif";
const ALLOWED = new Set(ACCEPT.split(","));
const MAX_BYTES = 1024 * 1024; // 1 MB

/** Org logo upload — white-labels the workspace HQ chrome (rail tile + HQ
 *  heading). Org-level; one logo shown across all the org's workspaces. */
export default function OrgLogoUpload({ org }: { org: Organization }) {
  const { workspace } = useCurrentWorkspace();
  const { setOrg } = useCurrentOrg();
  const uploadLogo = useUploadOrgLogo();
  const deleteLogo = useDeleteOrgLogo();
  const fileRef = useRef<HTMLInputElement>(null);
  const [broken, setBroken] = useState(false);

  // The logo is served through the workspace endpoint (org logo first, then
  // the code-first file). `updated_at` busts the cache after a change.
  const previewUrl =
    workspace?.id && !broken ? workspaceLogoUrl(workspace.id, org.updated_at) : null;

  const refreshOrg = async () => {
    try {
      setOrg(await OrganizationService.getOrg(org.id));
    } catch {
      // Non-fatal: the upload/delete already succeeded; the new logo shows
      // on the next load even if this refresh fails.
    }
  };

  const handleFile = async (file: File | undefined) => {
    if (!file) return;
    if (!ALLOWED.has(file.type)) {
      toast.error("Use a PNG, JPG, SVG, or WebP image");
      return;
    }
    if (file.size > MAX_BYTES) {
      toast.error("Logo must be under 1 MB");
      return;
    }
    try {
      await uploadLogo.mutateAsync({ orgId: org.id, file });
      setBroken(false);
      await refreshOrg();
      toast.success("Logo updated");
    } catch {
      toast.error("Failed to upload logo");
    }
  };

  const handleRemove = async () => {
    try {
      await deleteLogo.mutateAsync(org.id);
      await refreshOrg();
      setBroken(true);
      toast.success("Logo removed");
    } catch {
      toast.error("Failed to remove logo");
    }
  };

  const busy = uploadLogo.isPending || deleteLogo.isPending;

  return (
    <div className='space-y-2'>
      <Label>Logo</Label>
      <div className='flex items-center gap-4 rounded-lg border p-4'>
        <div className='flex h-20 w-20 shrink-0 items-center justify-center overflow-hidden rounded-lg border bg-muted'>
          {previewUrl ? (
            <img
              key={previewUrl}
              src={previewUrl}
              alt={org.name}
              onError={() => setBroken(true)}
              className='h-full w-full object-contain'
            />
          ) : (
            <span className='font-bold text-3xl text-muted-foreground'>
              {org.name.slice(0, 1).toUpperCase()}
            </span>
          )}
        </div>
        <div className='flex min-w-0 flex-1 flex-col gap-2'>
          <input
            ref={fileRef}
            type='file'
            accept={ACCEPT}
            className='hidden'
            onChange={(e) => {
              // handleFile toasts its own failures and cannot reject.
              void handleFile(e.target.files?.[0]);
              e.target.value = "";
            }}
          />
          <div className='flex flex-wrap items-center gap-2'>
            <Button variant='outline' onClick={() => fileRef.current?.click()} disabled={busy}>
              <ImageUp className='h-4 w-4' />
              {previewUrl ? "Replace" : "Upload"}
            </Button>
            {previewUrl && (
              <Button
                variant='ghost'
                onClick={handleRemove}
                disabled={busy}
                className='text-muted-foreground hover:text-destructive'
              >
                <Trash2 className='h-4 w-4' />
                Remove
              </Button>
            )}
          </div>
          <p className='text-muted-foreground text-xs'>
            PNG, JPG, SVG, or WebP, up to 1 MB. White-labels your workspace — shown on the home page
            and the sidebar.
          </p>
        </div>
      </div>
    </div>
  );
}
