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
import { useCreateClientOrg } from "@/hooks/api/partners";

const slugify = (s: string) =>
  s
    .toLowerCase()
    .trim()
    .replace(/[^a-z0-9]+/g, "-")
    .replace(/^-+|-+$/g, "")
    .slice(0, 63);

const isValidEmail = (s: string) => /^[^\s@]+@[^\s@]+\.[^\s@]+$/.test(s);

/**
 * Onboard a client — the thing a reseller channel is for, and the thing partners
 * previously had to open a support ticket to get.
 *
 * Requires `create_orgs`. Creating a client is safe to delegate in a way that
 * *attaching an existing org* is not: a brand-new org affects nobody else's
 * tenant. Attaching a live one stays with Oxy.
 */
export default function CreateClientDialog({ partnerId }: { partnerId: string }) {
  const [open, setOpen] = useState(false);
  const [name, setName] = useState("");
  const [slug, setSlug] = useState("");
  const [ownerEmail, setOwnerEmail] = useState("");
  const [slugTouched, setSlugTouched] = useState(false);
  const create = useCreateClientOrg(partnerId);

  const reset = () => {
    setName("");
    setSlug("");
    setOwnerEmail("");
    setSlugTouched(false);
  };

  const submit = () =>
    create.mutate(
      {
        name: name.trim(),
        slug: slug.trim(),
        ...(ownerEmail.trim() ? { owner_email: ownerEmail.trim() } : {})
      },
      {
        onSuccess: () => {
          setOpen(false);
          reset();
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
        <Button size='sm' data-testid='partner-new-client'>
          New client
        </Button>
      </DialogTrigger>
      <DialogContent className={`${DESIGN_REFERENCE} max-w-md`}>
        <DialogHeader>
          <DialogTitle>Onboard a client</DialogTitle>
          <DialogDescription>
            Creates the organization and puts it under your management, in one step.
          </DialogDescription>
        </DialogHeader>

        <div className='space-y-3'>
          <div className='space-y-1'>
            <Label htmlFor='client-name'>Organization name</Label>
            <Input
              id='client-name'
              data-testid='partner-new-client-name'
              value={name}
              placeholder='Northwind Traders'
              onChange={(e) => {
                setName(e.target.value);
                if (!slugTouched) setSlug(slugify(e.target.value));
              }}
            />
          </div>
          <div className='space-y-1'>
            <Label htmlFor='client-slug'>Slug</Label>
            <Input
              id='client-slug'
              data-testid='partner-new-client-slug'
              value={slug}
              placeholder='northwind'
              onChange={(e) => {
                setSlugTouched(true);
                setSlug(slugify(e.target.value));
              }}
            />
          </div>
          <div className='space-y-1'>
            <Label htmlFor='client-owner'>First owner (optional)</Label>
            <Input
              id='client-owner'
              data-testid='partner-new-client-owner'
              type='email'
              value={ownerEmail}
              placeholder='owner@northwind.com'
              onChange={(e) => setOwnerEmail(e.target.value)}
            />
            <p className='text-muted-foreground text-xs'>
              The client owns their organization from day one — you administer it, you don&apos;t
              own it. If they already have an Oxy account they become owner now; otherwise we email
              them an invite to claim ownership.
            </p>
          </div>
        </div>

        <DialogFooter>
          <Button
            variant='ghost'
            onClick={() => setOpen(false)}
            data-testid='partner-new-client-cancel'
          >
            Cancel
          </Button>
          <Button
            disabled={
              !name.trim() ||
              !slug.trim() ||
              (!!ownerEmail.trim() && !isValidEmail(ownerEmail.trim())) ||
              create.isPending
            }
            onClick={submit}
            data-testid='partner-new-client-submit'
          >
            Create client
          </Button>
        </DialogFooter>
      </DialogContent>
    </Dialog>
  );
}
