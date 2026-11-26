import { Skeleton } from "@/components/ui/shadcn/skeleton";
import { Switch } from "@/components/ui/shadcn/switch";
import { Table, TableBody, TableCell, TableHeader, TableRow } from "@/components/ui/shadcn/table";
import { usePartnerPeople, useSetPersonAccess } from "@/hooks/api/partners";
import { ADMIN_HEADER_ROW_CLASS, AdminTh } from "@/pages/admin/components/AdminTable";
import { orgRoleKind, RoleBadge } from "@/pages/admin/components/RoleBadge";
import { type AdminPartnerCapabilities, CAPABILITY_LABELS } from "@/types/adminPartners";
import type { PartnerPerson } from "@/types/partners";
import PageShell from "../components/PageShell";
import { usePartnerConsole } from "../context";

/**
 * Your team — who at your organization is a partner **operator** (can act on your
 * clients).
 *
 * Access is one thing, bounded by your **ceiling**: an operator reaches every
 * client you manage and can do everything Oxy granted this partnership — no more.
 * There are no per-person roles, so the only decision per person is in or out.
 * Owners and admins of your org make it; the toggle applies immediately.
 */
export default function PartnerTeam() {
  const { active } = usePartnerConsole();
  const partnerId = active.partner_id;

  const { data: people, isLoading, error } = usePartnerPeople(partnerId);
  const setAccess = useSetPersonAccess(partnerId);

  return (
    <PageShell
      eyebrow={active.name}
      title='Team'
      description='Who at your organization can act on your clients. Every operator reaches every client, within your ceiling.'
      testId='partner-team-page'
    >
      <CeilingSummary ceiling={active.capabilities} />
      {isLoading ? (
        <Skeleton className='h-64 w-full' />
      ) : error ? (
        <p className='text-destructive text-sm'>Failed to load your team.</p>
      ) : !people?.length ? (
        <p className='text-muted-foreground text-sm'>Nobody here yet.</p>
      ) : (
        <Table>
          <TableHeader>
            <TableRow className={ADMIN_HEADER_ROW_CLASS}>
              <AdminTh>Person</AdminTh>
              <AdminTh>Org role</AdminTh>
              <AdminTh className='text-right'>Partner access</AdminTh>
            </TableRow>
          </TableHeader>
          <TableBody>
            {people.map((p) => (
              <PersonRow
                key={p.org_member_id}
                person={p}
                disabled={setAccess.isPending}
                onToggle={(hasAccess) =>
                  setAccess.mutate({ orgMemberId: p.org_member_id, hasAccess })
                }
              />
            ))}
          </TableBody>
        </Table>
      )}
    </PageShell>
  );
}

function PersonRow({
  person,
  disabled,
  onToggle
}: {
  person: PartnerPerson;
  disabled: boolean;
  onToggle: (hasAccess: boolean) => void;
}) {
  return (
    <TableRow className='border-border-subtle'>
      <TableCell className='max-w-0'>
        <div className='truncate font-medium text-sm'>{person.name ?? person.email}</div>
        <div className='truncate text-muted-foreground text-xs'>{person.email}</div>
      </TableCell>
      <TableCell>
        <RoleBadge kind={orgRoleKind(person.org_role)} />
      </TableCell>
      <TableCell className='text-right'>
        <Switch
          checked={person.has_access}
          disabled={disabled}
          onCheckedChange={onToggle}
          aria-label={`Partner access for ${person.email}`}
          data-testid={`partner-team-access-${person.user_id}`}
        />
      </TableCell>
    </TableRow>
  );
}

/** What "partner access" concretely grants — your ceiling, in words. */
function CeilingSummary({ ceiling }: { ceiling: AdminPartnerCapabilities }) {
  const granted = (Object.keys(CAPABILITY_LABELS) as (keyof AdminPartnerCapabilities)[]).filter(
    (cap) => ceiling[cap]
  );
  return (
    <p className='mb-3 text-muted-foreground text-xs'>
      An operator can:{" "}
      {granted.length === 0 ? (
        <span className='italic'>nothing — your ceiling grants no capabilities</span>
      ) : (
        <span className='text-foreground/80'>
          {granted.map((cap) => CAPABILITY_LABELS[cap]).join(" · ")}
        </span>
      )}
    </p>
  );
}
