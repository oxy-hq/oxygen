import { Database } from "lucide-react";
import { CanWorkspaceAdmin } from "@/components/auth/Can";
import WorkspaceRepositoryNotice from "@/components/WorkspaceRepositoryNotice";
import NoAccessNotice from "../../../components/NoAccessNotice";
import SectionHeader from "../../../components/SectionHeader";
import DatabaseTable from "./DatabaseTable";

/**
 * The workspace's database connections, read-only. A connection is declared
 * in `config.yml`, so adding or changing one is a change to the workspace's
 * repository.
 */
export default function Databases() {
  // Gated as a whole, matching the nav: a member does not get a read-only list
  // of every warehouse connection in the workspace.
  return (
    <CanWorkspaceAdmin
      fallback={
        <NoAccessNotice>You need workspace admin access to manage databases.</NoAccessNotice>
      }
    >
      <div className='flex flex-col gap-5'>
        <SectionHeader icon={Database} title='Databases' />
        <WorkspaceRepositoryNotice />
        <DatabaseTable />
      </div>
    </CanWorkspaceAdmin>
  );
}
