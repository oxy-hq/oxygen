import { Plus } from "lucide-react";
import { useState } from "react";
import { Button } from "@/components/ui/shadcn/button";
import { useTeams } from "@/hooks/api/appAccess";
import type { Team } from "@/types/appAccess";
import type { Organization, OrgRole } from "@/types/organization";
import { SectionActions } from "../../../components/SectionChrome";
import { TeamEditor } from "./components/TeamEditor";

import { TeamList } from "./components/TeamList";

interface TeamsSectionProps {
  org: Organization;
  viewerRole: OrgRole;
}

/**
 * Org teams — named groups you grant apps to.
 *
 * Teams exist for one reason today: granting custom-app access without picking the
 * same eight people for every app. The copy says that plainly rather than
 * describing them as a general-purpose primitive they aren't yet.
 */
export default function TeamsSection({ org, viewerRole }: TeamsSectionProps) {
  const orgId = org.id;
  const { data: teams, isPending, isError } = useTeams(orgId);
  const [editing, setEditing] = useState<Team | null>(null);
  const [creating, setCreating] = useState(false);

  const canManage = viewerRole === "owner" || viewerRole === "admin";

  if (!canManage) {
    return (
      <div className='flex items-center justify-center py-12'>
        <p className='text-muted-foreground text-sm'>
          You need to be an organization owner or admin to manage teams.
        </p>
      </div>
    );
  }

  return (
    <div className='flex flex-col gap-5'>
      <SectionActions>
        <Button size='sm' className='gap-1.5' onClick={() => setCreating(true)}>
          <Plus className='size-4' aria-hidden='true' />
          New team
        </Button>
      </SectionActions>

      <TeamList
        orgId={orgId}
        teams={teams ?? []}
        isPending={isPending}
        isError={isError}
        onEdit={setEditing}
        onCreate={() => setCreating(true)}
      />

      <TeamEditor
        orgId={orgId}
        team={editing}
        open={creating || editing !== null}
        onOpenChange={(open) => {
          if (!open) {
            setCreating(false);
            setEditing(null);
          }
        }}
      />
    </div>
  );
}
