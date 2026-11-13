import type React from "react";
import TableContentWrapper from "@/components/settings/components/TableContentWrapper";
import TableWrapper from "@/components/settings/components/TableWrapper";
import { Table, TableBody, TableHead, TableHeader, TableRow } from "@/components/ui/shadcn/table";
import useDatabases from "@/hooks/api/databases/useDatabases";
import DatabaseRow from "./DatabaseRow";

const DatabaseTable: React.FC = () => {
  const { data: databases = [], isLoading, error, refetch } = useDatabases();

  return (
    <TableWrapper>
      <Table>
        <TableHeader>
          <TableRow>
            <TableHead>Database</TableHead>
            <TableHead>Datasets</TableHead>
          </TableRow>
        </TableHeader>
        <TableBody>
          <TableContentWrapper
            isEmpty={databases.length === 0}
            loading={isLoading}
            colSpan={3}
            noFoundTitle='No databases found'
            noFoundDescription='Add a database to get started'
            error={error?.message}
            onRetry={() => refetch()}
          >
            {databases.map((database) => (
              <DatabaseRow key={database.name} database={database} />
            ))}
          </TableContentWrapper>
        </TableBody>
      </Table>
    </TableWrapper>
  );
};

export default DatabaseTable;
