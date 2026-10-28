import { ExternalLink, Wrench } from "lucide-react";
import { Link } from "react-router-dom";
import { Button } from "@/components/ui/shadcn/button";
import { Switch } from "@/components/ui/shadcn/switch";
import {
  useAdminFactoryAccess,
  useSetAdminFactoryAccess
} from "@/hooks/api/adminTenants/useAdminWorkspaces";
import ROUTES from "@/libs/utils/routes";
import type { AdminFactoryAccessMember } from "@/services/api/adminTenants";
import { AdminAsync } from "../../../components/AdminAsync";
import { AdminEmptyState } from "../../../components/AdminEmptyState";
import { AdminSectionLabel } from "../../../components/AdminSectionLabel";
import { AdminStatusPill } from "../../../components/AdminStatusPill";

/**
 * Who an operator has let into this workspace's Oxygen Factory (the IDE and back
 * office). Oxy staff and the owners and admins of the customer's org always reach it;
 * every other member needs the grant here. Only members of the org can be granted:
 * a grant whose holder has left the org opens nothing, and is listed so it can be
 * removed.
 *
 * With the org's slug it also links to the workspace's Factory (its IDE route), so
 * an operator opens it from here rather than through a customer's screens.
 */
export default function FactoryAccessPanel({
  workspaceId,
  orgSlug
}: {
  workspaceId: string;
  orgSlug?: string | null;
}) {
  const access = useAdminFactoryAccess(workspaceId);
  const set = useSetAdminFactoryAccess(workspaceId);

  return (
    <section className='space-y-3' data-testid='factory-access-panel'>
      {orgSlug ? (
        <div className='flex justify-end'>
          <Button asChild variant='outline' size='sm'>
            <Link
              to={ROUTES.ORG(orgSlug).WORKSPACE(workspaceId).IDE.ROOT}
              data-testid='factory-access-open-factory'
            >
              <ExternalLink className='size-3.5' />
              Open Factory
            </Link>
          </Button>
        </div>
      ) : null}
      <AdminAsync
        query={access}
        noun='Factory access'
        rows={4}
        isEmpty={(d) => d.members.length === 0}
        empty={
          <AdminEmptyState
            icon={Wrench}
            title='No members'
            description="Nobody in this workspace's organization can be let into its Factory yet."
          />
        }
      >
        {(data) => (
          <>
            <AdminSectionLabel
              trailing={
                <span className='tabular-nums'>
                  {data.members.filter((m) => m.granted).length.toLocaleString()} granted
                </span>
              }
            >
              Factory access
            </AdminSectionLabel>
            <p className='text-muted-foreground text-xs'>
              {data.enforced
                ? "Enforcement is on: Oxy staff, the organization's owners and admins, and the members granted here can use this workspace's Factory. Owners and admins already have access without a grant."
                : "Enforcement is off (feature flag apps_first_experience, which also switches on the customer home): everyone can still use the Factory. Owners and admins already have access without a grant; grant the other members who use it today, then turn it on."}
            </p>
            <ul className='divide-y divide-border/60 rounded-lg border border-border/60 bg-card'>
              {data.members.map((m) => (
                <MemberRow
                  key={m.user_id}
                  member={m}
                  pending={set.isPending && set.variables?.userId === m.user_id}
                  onChange={(granted) => set.mutate({ userId: m.user_id, granted })}
                />
              ))}
            </ul>
          </>
        )}
      </AdminAsync>
    </section>
  );
}

function MemberRow({
  member,
  pending,
  onChange
}: {
  member: AdminFactoryAccessMember;
  pending: boolean;
  onChange: (granted: boolean) => void;
}) {
  const left = member.org_role === null;
  // An owner or admin of the org reaches the Factory by that role; a grant adds
  // nothing for them while they hold it.
  const byRole = member.org_role === "owner" || member.org_role === "admin";
  return (
    <li
      className='flex items-center justify-between gap-4 px-4 py-3'
      data-testid={`factory-access-row-${member.user_id}`}
    >
      <div className='min-w-0'>
        <div className='truncate font-medium text-sm'>{member.name || member.email}</div>
        <div className='truncate text-muted-foreground text-xs'>
          {member.email}
          {member.granted && member.granted_at
            ? ` · granted ${new Date(member.granted_at).toLocaleDateString()}${
                member.granted_by ? ` by ${member.granted_by}` : ""
              }`
            : ""}
        </div>
      </div>
      <div className='flex shrink-0 items-center gap-3'>
        {left ? <AdminStatusPill tone='warn' label='Left the org' /> : null}
        {byRole ? <AdminStatusPill tone='muted' label='Has access by role' /> : null}
        {member.org_role ? <AdminStatusPill tone='muted' label={member.org_role} /> : null}
        <Switch
          checked={member.granted}
          disabled={pending || (left && !member.granted)}
          onCheckedChange={onChange}
          aria-label={`Factory access for ${member.email}`}
          data-testid={`factory-access-switch-${member.user_id}`}
        />
      </div>
    </li>
  );
}
