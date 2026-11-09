import type React from "react";
import { useCallback } from "react";
import { TableCell, TableRow } from "@/components/ui/shadcn/table";
import { useDatabaseSync } from "@/hooks/api/databases/useDatabaseSync";
import useIdeRetired from "@/hooks/useIdeRetired";
import useDatabaseOperation from "@/stores/useDatabaseOperation";
import type { DatabaseInfo as Database } from "@/types/database";
import Actions from "./Actions";
import { DatabaseInfo, DatasetInfo } from "./Info";

interface DatabaseTableRowProps {
  database: Database;
}

const DatabaseRow: React.FC<DatabaseTableRowProps> = ({ database }) => {
  const syncMutation = useDatabaseSync();
  const handleSync = useCallback(
    (datasets?: string[]) => {
      syncMutation.mutate({
        database: database.name,
        options: {
          ...(datasets && datasets.length > 0 && { datasets })
        }
      });
    },
    [database.name, syncMutation]
  );
  const { isSyncing } = useDatabaseOperation();
  const isCurrentlySyncing = isSyncing(database.name);
  // IDE retired: sync writes the working copy, which the server refuses.
  const ideRetired = useIdeRetired();

  return (
    <TableRow>
      <TableCell data-label='Database' className='font-medium'>
        <DatabaseInfo database={database} />
      </TableCell>
      <TableCell data-label='Datasets'>
        <DatasetInfo datasets={database.datasets} />
      </TableCell>
      <TableCell className='max-w-md'>
        {!ideRetired && (
          <Actions database={database} isLoading={isCurrentlySyncing} onSync={handleSync} />
        )}
      </TableCell>
    </TableRow>
  );
};

export default DatabaseRow;
