import type React from "react";
import TableContentWrapper from "@/components/settings/components/TableContentWrapper";
import TableWrapper from "@/components/settings/components/TableWrapper";
import { Table, TableBody, TableHead, TableHeader, TableRow } from "@/components/ui/shadcn/table";
import useApiKeys from "@/hooks/api/apiKeys/useApiKeys";
import LegacyKeyRow from "./components/LegacyKeyRow";

/** This workspace's legacy API keys, from `GET /{workspaceId}/api-keys`. */
const LegacyKeyTable: React.FC = () => {
  const { data, isLoading, error, refetch } = useApiKeys();
  const apiKeys = data?.api_keys ?? [];

  return (
    <TableWrapper>
      <Table className='text-xs' data-testid='legacy-api-keys-table'>
        <TableHeader>
          <TableRow>
            <TableHead>Name</TableHead>
            <TableHead>Expiry</TableHead>
            <TableHead>Last used</TableHead>
            <TableHead>Created</TableHead>
            <TableHead>
              <span className='sr-only'>Actions</span>
            </TableHead>
          </TableRow>
        </TableHeader>
        <TableBody>
          <TableContentWrapper
            isEmpty={apiKeys.length === 0}
            loading={isLoading}
            colSpan={5}
            error={error ? "Couldn't load the legacy API keys." : undefined}
            noFoundTitle='No legacy API keys'
            noFoundDescription='New credentials are created as API tokens.'
            onRetry={refetch}
          >
            {apiKeys.map((apiKey) => (
              <LegacyKeyRow key={apiKey.id} apiKey={apiKey} />
            ))}
          </TableContentWrapper>
        </TableBody>
      </Table>
    </TableWrapper>
  );
};

export default LegacyKeyTable;
