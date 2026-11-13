import type React from "react";
import { TableCell, TableRow } from "@/components/ui/shadcn/table";
import type { DatabaseInfo as Database } from "@/types/database";
import { DatabaseInfo, DatasetInfo } from "./Info";

interface DatabaseTableRowProps {
  database: Database;
}

const DatabaseRow: React.FC<DatabaseTableRowProps> = ({ database }) => (
  <TableRow>
    <TableCell data-label='Database' className='font-medium'>
      <DatabaseInfo database={database} />
    </TableCell>
    <TableCell data-label='Datasets'>
      <DatasetInfo datasets={database.datasets} />
    </TableCell>
  </TableRow>
);

export default DatabaseRow;
