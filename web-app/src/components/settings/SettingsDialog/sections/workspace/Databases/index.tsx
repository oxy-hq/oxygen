import { Database, Plus, Trash2 } from "lucide-react";
import { useCallback, useState } from "react";
import { CanWorkspaceAdmin } from "@/components/auth/Can";
import RetiredIdeNotice from "@/components/RetiredIdeNotice";
import { Button } from "@/components/ui/shadcn/button";
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogHeader,
  DialogTitle
} from "@/components/ui/shadcn/dialog";
import { Separator } from "@/components/ui/shadcn/separator";
import { Spinner } from "@/components/ui/shadcn/spinner";
import { useDataClean } from "@/hooks/api/databases/useDataClean";
import useIdeRetired from "@/hooks/useIdeRetired";
import useDatabaseOperation from "@/stores/useDatabaseOperation";
import NoAccessNotice from "../../../components/NoAccessNotice";
import SectionHeader from "../../../components/SectionHeader";
import { AddDatabaseForm } from "./AddDatabaseForm";
import DatabaseTable from "./DatabaseTable";
import { EmbeddingsManagement } from "./EmbeddingsManagement";

export default function Databases() {
  const { isCleaning } = useDatabaseOperation();
  const cleanMutation = useDataClean();
  const cleaningInProgress = isCleaning();
  const [isAddDialogOpen, setIsAddDialogOpen] = useState(false);
  // IDE retired: adding a database, resetting state and embeddings all write the
  // working copy, which the server refuses; the list stays.
  const ideRetired = useIdeRetired();

  const handleCleanAll = useCallback(() => {
    cleanMutation.mutate("all");
  }, [cleanMutation]);

  const handleAddDatabaseSuccess = () => {
    setIsAddDialogOpen(false);
  };

  const handleCloseDialog = () => {
    setIsAddDialogOpen(false);
  };

  const headerActions = ideRetired ? null : (
    <CanWorkspaceAdmin>
      <div className='flex gap-2'>
        <Button size='sm' variant='outline' onClick={() => setIsAddDialogOpen(true)}>
          <Plus />
          Add Database
        </Button>
        <Button size='sm' variant='outline' onClick={handleCleanAll} disabled={cleaningInProgress}>
          {cleaningInProgress ? (
            <Spinner />
          ) : (
            <>
              <Trash2 />
              Reset Oxygen State
            </>
          )}
        </Button>
      </div>
    </CanWorkspaceAdmin>
  );

  // The section used to hide only its buttons, leaving a member a read-only
  // list of every warehouse connection in the workspace. Gate the whole
  // section instead, matching the nav — the inner `CanWorkspaceAdmin` on the
  // actions is now redundant but harmless, and keeps the block self-describing.
  return (
    <CanWorkspaceAdmin
      fallback={
        <NoAccessNotice>You need workspace admin access to manage databases.</NoAccessNotice>
      }
    >
      <div className='flex flex-col gap-5'>
        <SectionHeader icon={Database} title='Databases' actions={headerActions} />

        {ideRetired && <RetiredIdeNotice />}

        <DatabaseTable />

        {!ideRetired && (
          <>
            <Separator />
            <EmbeddingsManagement />
          </>
        )}

        <Dialog open={isAddDialogOpen} onOpenChange={setIsAddDialogOpen}>
          <DialogContent className='flex max-h-[85vh] max-w-2xl flex-col overflow-hidden p-0'>
            <DialogHeader className='p-6 pb-0'>
              <DialogTitle>Add Database Connection</DialogTitle>
              <DialogDescription>Configure a new database connection</DialogDescription>
            </DialogHeader>
            <div className='min-h-0 flex-1 overflow-auto p-6 pt-0'>
              <AddDatabaseForm onSuccess={handleAddDatabaseSuccess} onCancel={handleCloseDialog} />
            </div>
          </DialogContent>
        </Dialog>
      </div>
    </CanWorkspaceAdmin>
  );
}
