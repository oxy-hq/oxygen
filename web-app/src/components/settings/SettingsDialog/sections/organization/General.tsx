import { isAxiosError } from "axios";
import { useState } from "react";
import { useNavigate } from "react-router-dom";
import { toast } from "sonner";
import { CanOrgAdmin, CanOrgOwner } from "@/components/auth/Can";
import { ConfirmDeleteDialog } from "@/components/ui/ConfirmDeleteDialog";
import { Button } from "@/components/ui/shadcn/button";
import { Input } from "@/components/ui/shadcn/input";
import { Label } from "@/components/ui/shadcn/label";
import { useDeleteOrg, useUpdateOrg } from "@/hooks/api/organizations";
import { clearLastOrgSlug } from "@/libs/utils/lastWorkspace";
import ROUTES from "@/libs/utils/routes";
import useCurrentOrg from "@/stores/useCurrentOrg";
import type { Organization } from "@/types/organization";
import NoAccessNotice from "../../components/NoAccessNotice";
import OrgLogoUpload from "./OrgLogoUpload";
import PartnerPublishConsent from "./PartnerPublishConsent";

interface GeneralSectionProps {
  org: Organization;
  onClose: () => void;
}

export default function GeneralSection({ org, onClose }: GeneralSectionProps) {
  const navigate = useNavigate();
  const [name, setName] = useState(org.name);
  const [slug, setSlug] = useState(org.slug);
  const updateOrg = useUpdateOrg();
  const deleteOrg = useDeleteOrg();
  const { setOrg, clearOrg } = useCurrentOrg();
  const [deleteOpen, setDeleteOpen] = useState(false);
  const [slugError, setSlugError] = useState("");

  const handleSave = async () => {
    setSlugError("");
    try {
      const updated = await updateOrg.mutateAsync({
        orgId: org.id,
        data: { name: name.trim(), slug: slug.trim() }
      });
      setOrg(updated);
      toast.success("Organization updated");
      if (updated.slug !== org.slug) {
        onClose();
        navigate(ROUTES.ORG(updated.slug).ROOT);
      }
    } catch (err) {
      if (isAxiosError(err)) {
        if (err.response?.status === 409) {
          setSlugError("This slug is already taken. Please choose a different one.");
          return;
        }
        if (err.response?.status === 422) {
          setSlugError("This slug is reserved. Please choose a different one.");
          return;
        }
      }
      toast.error("Failed to update organization");
    }
  };

  const handleDelete = async () => {
    try {
      await deleteOrg.mutateAsync(org.id);
      clearOrg();
      // Drop the persisted slug so PostLoginDispatcher can't match it against
      // stale cache and briefly re-select the deleted org.
      clearLastOrgSlug();
      toast.success("Organization deleted");
      onClose();
      navigate(ROUTES.ROOT);
    } catch {
      toast.error("Failed to delete organization");
    }
  };

  const hasChanges = name.trim() !== org.name || slug.trim() !== org.slug;

  // Every block below is admin-or-owner only, so a member used to get an empty
  // panel. Gate the whole section once, with a reason, and let the inner
  // `CanOrgOwner` still narrow the Danger Zone to owners.
  return (
    <CanOrgAdmin
      fallback={
        <NoAccessNotice>
          You need organization admin access to manage organization settings.
        </NoAccessNotice>
      }
    >
      <div className='space-y-8'>
        <div className='space-y-4'>
          <OrgLogoUpload org={org} />
          <div className='space-y-2'>
            <Label htmlFor='org-name'>Organization name</Label>
            <Input id='org-name' value={name} onChange={(e) => setName(e.target.value)} />
          </div>
          <div className='space-y-2'>
            <Label htmlFor='org-slug'>URL slug</Label>
            <Input
              id='org-slug'
              value={slug}
              onChange={(e) => {
                setSlug(e.target.value);
                setSlugError("");
              }}
            />
            {slugError && <p className='text-destructive text-sm'>{slugError}</p>}
          </div>
          <Button onClick={handleSave} disabled={!hasChanges || updateOrg.isPending}>
            {updateOrg.isPending ? "Saving…" : "Save"}
          </Button>
        </div>

        <PartnerPublishConsent orgId={org.id} />

        <CanOrgOwner>
          <div className='space-y-4 rounded-lg border border-destructive/50 p-4'>
            <h3 className='font-medium text-destructive'>Danger zone</h3>
            <p className='text-muted-foreground text-sm'>
              Deleting this organization will permanently remove all workspaces, members, and data.
            </p>
            <Button variant='destructive' onClick={() => setDeleteOpen(true)}>
              Delete organization
            </Button>
          </div>
        </CanOrgOwner>

        <ConfirmDeleteDialog
          open={deleteOpen}
          onOpenChange={setDeleteOpen}
          title='Delete this entire organization permanently?'
          description='This action cannot be undone. This will permanently delete the organization, including all workspaces, members, and data. Please type the name of the organization to confirm.'
          confirmationName={org.name}
          confirmButtonLabel='Permanently delete organization'
          onConfirm={handleDelete}
          isPending={deleteOrg.isPending}
        />
      </div>
    </CanOrgAdmin>
  );
}
