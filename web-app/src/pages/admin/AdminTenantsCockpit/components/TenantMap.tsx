import { Building2, ChevronDown, ChevronRight, Handshake, User } from "lucide-react";
import { useState } from "react";
import { Badge } from "@/components/ui/shadcn/badge";
import { Skeleton } from "@/components/ui/shadcn/skeleton";
import { useAdminPartnerDetail, useAdminPartners } from "@/hooks/api/adminPartners";
import { useAdminOrgDetail, useAdminOrgsList } from "@/hooks/api/adminTenants/index";
import type { TenantType } from "../useTenantSelection";

/**
 * The relationship map: Partner ▸ Orgs ▸ Members as a lazily-expanded tree, plus
 * an "unmanaged" bucket for orgs with no partner. This is the org-chart view of
 * who-manages-whom. Clicking any node label opens its dossier (flips to list).
 */
export default function TenantMap({ onFocus }: { onFocus: (t: TenantType, id: string) => void }) {
  const { data: partners, isPending } = useAdminPartners();

  return (
    <div className='mx-auto max-w-4xl space-y-6 p-6'>
      <section>
        <h3 className='mb-2 flex items-center gap-1.5 font-medium text-muted-foreground text-xs uppercase tracking-wide'>
          <Handshake className='size-3.5' /> Partners
        </h3>
        {isPending ? (
          <Skeleton className='h-24 w-full' />
        ) : !partners?.length ? (
          <p className='text-muted-foreground text-xs'>No partners yet.</p>
        ) : (
          <div className='space-y-1'>
            {partners.map((p) => (
              <PartnerNode
                key={p.org_id}
                id={p.org_id}
                name={p.name}
                orgCount={p.managed_count}
                status={p.status}
                onFocus={onFocus}
              />
            ))}
          </div>
        )}
      </section>

      <UnmanagedOrgs onFocus={onFocus} />
    </div>
  );
}

/** A row that toggles a child subtree; the label opens the entity's dossier. */
function TreeRow({
  depth,
  open,
  hasChildren,
  onToggle,
  icon,
  label,
  sub,
  trailing,
  onOpen
}: {
  depth: number;
  open?: boolean;
  hasChildren?: boolean;
  onToggle?: () => void;
  icon: React.ReactNode;
  label: string;
  sub?: string;
  trailing?: React.ReactNode;
  onOpen?: () => void;
}) {
  return (
    <div
      className='flex items-center gap-1 py-1 pr-2 hover:bg-muted/50'
      style={{ paddingLeft: `${depth * 20 + 4}px` }}
    >
      {hasChildren ? (
        <button type='button' onClick={onToggle} className='shrink-0 text-muted-foreground'>
          {open ? <ChevronDown className='size-3.5' /> : <ChevronRight className='size-3.5' />}
        </button>
      ) : (
        <span className='w-3.5 shrink-0' />
      )}
      <span className='shrink-0 text-muted-foreground'>{icon}</span>
      <button
        type='button'
        onClick={onOpen}
        className='flex min-w-0 flex-1 items-baseline gap-1.5 text-left'
      >
        <span className='truncate font-medium text-[13px]'>{label}</span>
        {sub && <span className='truncate text-[11px] text-muted-foreground'>{sub}</span>}
      </button>
      {trailing}
    </div>
  );
}

function PartnerNode({
  id,
  name,
  orgCount,
  status,
  onFocus
}: {
  id: string;
  name: string;
  orgCount: number;
  status: string;
  onFocus: (t: TenantType, id: string) => void;
}) {
  const [open, setOpen] = useState(false);
  const { data, isPending } = useAdminPartnerDetail(open ? id : undefined);
  return (
    <div>
      <TreeRow
        depth={0}
        open={open}
        hasChildren={orgCount > 0}
        onToggle={() => setOpen((o) => !o)}
        icon={<Handshake className='size-4 text-primary' />}
        label={name}
        sub={`${orgCount} orgs`}
        trailing={
          status !== "active" ? (
            <Badge variant='destructive' className='text-[10px]'>
              {status}
            </Badge>
          ) : undefined
        }
        onOpen={() => onFocus("partners", id)}
      />
      {open &&
        (isPending ? (
          <div className='pl-10'>
            <Skeleton className='h-6 w-48' />
          </div>
        ) : (
          data?.managed_orgs.map((o) => (
            <OrgNode
              key={o.org_id}
              id={o.org_id}
              name={o.org_name ?? o.org_id}
              slug={o.org_slug ?? undefined}
              depth={1}
              onFocus={onFocus}
            />
          ))
        ))}
    </div>
  );
}

function OrgNode({
  id,
  name,
  slug,
  depth,
  onFocus
}: {
  id: string;
  name: string;
  slug?: string;
  depth: number;
  onFocus: (t: TenantType, id: string) => void;
}) {
  const [open, setOpen] = useState(false);
  const { data, isPending } = useAdminOrgDetail(open ? id : undefined);
  return (
    <div>
      <TreeRow
        depth={depth}
        open={open}
        hasChildren
        onToggle={() => setOpen((o) => !o)}
        icon={<Building2 className='size-4' />}
        label={name}
        sub={slug}
        onOpen={() => onFocus("orgs", id)}
      />
      {open &&
        (isPending ? (
          <div style={{ paddingLeft: `${(depth + 1) * 20 + 24}px` }}>
            <Skeleton className='h-6 w-40' />
          </div>
        ) : !data?.owners.length ? (
          <p
            className='py-1 text-muted-foreground text-xs'
            style={{ paddingLeft: `${(depth + 1) * 20 + 24}px` }}
          >
            No members.
          </p>
        ) : (
          data.owners.map((m) => (
            <TreeRow
              key={m.user_id}
              depth={depth + 1}
              icon={<User className='size-3.5' />}
              label={m.name || m.email}
              sub={m.role}
              onOpen={() => onFocus("users", m.user_id)}
            />
          ))
        ))}
    </div>
  );
}

function UnmanagedOrgs({ onFocus }: { onFocus: (t: TenantType, id: string) => void }) {
  const { data, isPending } = useAdminOrgsList({});
  const orgs = (data ?? []).filter((o) => !o.partner);
  if (isPending) return <Skeleton className='h-16 w-full' />;
  if (!orgs.length) return null;
  return (
    <section>
      <h3 className='mb-2 flex items-center gap-1.5 font-medium text-muted-foreground text-xs uppercase tracking-wide'>
        <Building2 className='size-3.5' /> Unmanaged organizations ({orgs.length})
      </h3>
      <div className='space-y-1'>
        {orgs.map((o) => (
          <OrgNode key={o.id} id={o.id} name={o.name} slug={o.slug} depth={0} onFocus={onFocus} />
        ))}
      </div>
    </section>
  );
}
