import { Plus } from "lucide-react";
import { useState } from "react";
import { DESIGN_REFERENCE } from "@/components/ui/reference/tokens";
import { Button } from "@/components/ui/shadcn/button";
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogTitle,
  DialogTrigger
} from "@/components/ui/shadcn/dialog";
import { Input } from "@/components/ui/shadcn/input";
import { Label } from "@/components/ui/shadcn/label";
import { useCreateAdminOrg } from "@/hooks/api/adminTenants";

const slugify = (s: string) =>
  s
    .toLowerCase()
    .trim()
    .replace(/[^a-z0-9]+/g, "-")
    .replace(/^-+|-+$/g, "")
    .slice(0, 63);

const isValidEmail = (s: string) => /^[^\s@]+@[^\s@]+\.[^\s@]+$/.test(s);

/**
 * Provision a tenant: create the organization and onboard its owner in one step.
 * A known email is made Owner immediately; an unknown one is emailed an invite
 * to claim ownership (the server decides which — the UI just names the owner).
 */
export default function CreateOrgDialog({ onCreated }: { onCreated?: (orgId: string) => void }) {
  const [open, setOpen] = useState(false);
  const [name, setName] = useState("");
  const [slug, setSlug] = useState("");
  const [ownerEmail, setOwnerEmail] = useState("");
  const [slugTouched, setSlugTouched] = useState(false);
  const create = useCreateAdminOrg();

  const reset = () => {
    setName("");
    setSlug("");
    setOwnerEmail("");
    setSlugTouched(false);
  };

  const canSubmit =
    !!name.trim() && !!slug.trim() && isValidEmail(ownerEmail.trim()) && !create.isPending;

  const submit = () =>
    create.mutate(
      { name: name.trim(), slug: slug.trim(), owner_email: ownerEmail.trim() },
      {
        onSuccess: (data) => {
          setOpen(false);
          reset();
          onCreated?.(data.org.id);
        }
      }
    );

  return (
    <Dialog
      open={open}
      onOpenChange={(o) => {
        setOpen(o);
        if (!o) reset();
      }}
    >
      <DialogTrigger asChild>
        <Button size='sm' className='gap-1.5' data-testid='admin-create-org'>
          <Plus className='size-4' />
          New org
        </Button>
      </DialogTrigger>
      <DialogContent className={`${DESIGN_REFERENCE} max-w-md`}>
        <DialogHeader>
          <DialogTitle>Create organization</DialogTitle>
          <DialogDescription>
            Creates the organization and onboards its owner in one step.
          </DialogDescription>
        </DialogHeader>

        <div className='space-y-3'>
          <div className='space-y-1'>
            <Label htmlFor='admin-org-name'>Organization name</Label>
            <Input
              id='admin-org-name'
              value={name}
              placeholder='Northwind Traders'
              onChange={(e) => {
                setName(e.target.value);
                if (!slugTouched) setSlug(slugify(e.target.value));
              }}
            />
          </div>
          <div className='space-y-1'>
            <Label htmlFor='admin-org-slug'>Slug</Label>
            <Input
              id='admin-org-slug'
              value={slug}
              placeholder='northwind'
              onChange={(e) => {
                setSlugTouched(true);
                setSlug(slugify(e.target.value));
              }}
            />
          </div>
          <div className='space-y-1'>
            <Label htmlFor='admin-org-owner'>Owner email</Label>
            <Input
              id='admin-org-owner'
              type='email'
              value={ownerEmail}
              placeholder='owner@northwind.com'
              onChange={(e) => setOwnerEmail(e.target.value)}
            />
            <p className='text-muted-foreground text-xs'>
              If they already have an Oxy account they become owner now. Otherwise we email them an
              invite to claim ownership.
            </p>
          </div>
        </div>

        <DialogFooter>
          <Button variant='ghost' onClick={() => setOpen(false)}>
            Cancel
          </Button>
          <Button disabled={!canSubmit} onClick={submit}>
            Create organization
          </Button>
        </DialogFooter>
      </DialogContent>
    </Dialog>
  );
}
