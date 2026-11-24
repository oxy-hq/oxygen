import React from "react";
import TableContentWrapper from "@/components/settings/components/TableContentWrapper";
import TableWrapper from "@/components/settings/components/TableWrapper";
import { Table, TableBody, TableHead, TableHeader, TableRow } from "@/components/ui/shadcn/table";
import { useLogs } from "@/hooks/api/activityLogs/useLogs";
import LogRow from "./LogRow";

export default function ActivityLogs() {
  const { data: logsResponse, isLoading: loading, error, refetch } = useLogs();

  const logs = React.useMemo(() => {
    return logsResponse?.logs || [];
  }, [logsResponse?.logs]);

  const sortedLogs = React.useMemo(() => {
    return [...logs].sort(
      (a, b) => new Date(b.created_at).getTime() - new Date(a.created_at).getTime()
    );
  }, [logs]);

  return (
    <div className='flex flex-col gap-5'>
      <TableWrapper>
        <Table>
          <TableHeader>
            <TableRow>
              <TableHead>Thread</TableHead>
              <TableHead>Prompt</TableHead>
              <TableHead>Queries</TableHead>
              <TableHead>Created</TableHead>
            </TableRow>
          </TableHeader>
          <TableBody>
            <TableContentWrapper
              isEmpty={logs.length === 0}
              loading={loading}
              colSpan={4}
              noFoundTitle='No logs found'
              noFoundDescription='There are currently no activity logs available.'
              error={error?.message}
              onRetry={() => refetch()}
            >
              {sortedLogs.map((log) => (
                <LogRow key={log.id} log={log} />
              ))}
            </TableContentWrapper>
          </TableBody>
        </Table>
      </TableWrapper>
    </div>
  );
}
