import { DatabaseZap, Table2 } from "lucide-react";
import { useState } from "react";
import { useAppDataTables } from "@/hooks/api/customApps/useAppData";
import { AdminAsync } from "@/pages/admin/components/AdminAsync";
import { AdminEmptyState } from "@/pages/admin/components/AdminEmptyState";
import type { AppDataTables } from "@/types/appData";
import { environmentScope } from "../environmentScope";
import { absentCopy } from "./dataBrowserModel";
import { RowsGrid } from "./RowsGrid";
import { TableList } from "./TableList";

/**
 * The schema's tables, then one table's rows. Mounted only after "Browse data", and
 * once per environment.
 */
export const DataBrowser = ({ appId, environment }: { appId: string; environment: string }) => {
  const tables = useAppDataTables(appId, environment, true);

  return (
    <AdminAsync query={tables} noun='this app&rsquo;s tables' rows={3}>
      {(data) => <Loaded data={data} appId={appId} environment={environment} />}
    </AdminAsync>
  );
};

const Loaded = ({
  data,
  appId,
  environment
}: {
  data: AppDataTables;
  appId: string;
  environment: string;
}) => {
  const [selected, setSelected] = useState<string | null>(null);

  if (data.store.state === "absent") {
    // Why there is nothing, in the server's words: no store provisioned, no staging
    // branch (so staging reads production's, read-only), a sandbox still seeding.
    const copy = absentCopy(data.store, environmentScope(environment).label);
    return (
      <AdminEmptyState
        icon={DatabaseZap}
        title={copy.title}
        description={copy.description}
        className='py-8'
        data-testid='admin-app-data-absent'
      />
    );
  }

  if (data.tables.length === 0) {
    return (
      <AdminEmptyState
        icon={Table2}
        title={`The schema ${data.store.schema} has no tables yet.`}
        description='The app has a store here but its migrations have created nothing in it.'
        className='py-8'
        data-testid='admin-app-data-no-tables'
      />
    );
  }

  const table = data.tables.find((t) => t.name === selected) ?? null;
  return (
    <div className='grid min-w-0 gap-2 lg:grid-cols-[13rem_minmax(0,1fr)]'>
      <TableList
        schema={data.store.schema}
        tables={data.tables}
        selected={selected}
        onSelect={setSelected}
      />
      {table ? (
        // Keyed by table: its page and order mean nothing for another one.
        <RowsGrid
          key={table.name}
          appId={appId}
          environment={environment}
          table={table}
          textChars={data.caps.text_chars}
        />
      ) : (
        <p
          className='self-start border border-border/60 border-dashed px-3 py-6 text-center text-muted-foreground text-xs'
          data-testid='admin-app-data-pick'
        >
          Pick a table to read its rows.
        </p>
      )}
    </div>
  );
};
