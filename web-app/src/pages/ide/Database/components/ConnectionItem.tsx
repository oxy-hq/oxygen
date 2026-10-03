import {
  AlertCircle,
  ChevronDown,
  ChevronRight,
  Database as DatabaseIcon,
  RotateCw
} from "lucide-react";
import React from "react";
import {
  AirhouseLogo,
  BigQueryIcon,
  ClickHouseIcon,
  DuckDBIcon,
  MysqlIcon,
  PostgresIcon,
  RedshiftIcon,
  SnowflakeIcon
} from "@/components/icons";
import DomoIcon from "@/components/icons/Domoicon";
import { Button } from "@/components/ui/shadcn/button";
import { SidebarMenuButton, SidebarMenuItem } from "@/components/ui/shadcn/sidebar";
import useDatabaseSchema from "@/hooks/api/databases/useDatabaseSchema";
import { cn } from "@/libs/shadcn/utils";
import type { DatabaseInfo } from "@/types/database";
import { ConnectionSchemaContent } from "./ConnectionSchemaContent";

const getDatabaseIcon = (dbType: string, dialect: string) => {
  const iconProps = { className: "h-4 w-4 text-muted-foreground" };

  if (dbType === "airhouse" || dbType === "airhouse_managed") {
    return <AirhouseLogo className='h-4 w-4' />;
  }

  switch (dialect.toLowerCase()) {
    case "bigquery":
      return <BigQueryIcon {...iconProps} width={16} height={16} />;
    case "postgres":
    case "postgresql":
      return <PostgresIcon {...iconProps} width={16} height={16} />;
    case "mysql":
      return <MysqlIcon {...iconProps} width={16} height={16} />;
    case "snowflake":
      return <SnowflakeIcon {...iconProps} width={16} height={16} />;
    case "clickhouse":
      return <ClickHouseIcon {...iconProps} width={16} height={16} />;
    case "duckdb":
      return <DuckDBIcon {...iconProps} width={16} height={16} />;
    case "redshift":
      return <RedshiftIcon {...iconProps} width={16} height={16} />;
    case "domo":
      return <DomoIcon {...iconProps} width={16} height={16} />;
    default:
      return <DatabaseIcon className='h-4 w-4 text-muted-foreground' />;
  }
};

interface ConnectionItemProps {
  database: DatabaseInfo;
}

export const ConnectionItem: React.FC<ConnectionItemProps> = ({ database }) => {
  const [isOpen, setIsOpen] = React.useState(false);

  const {
    data: schema,
    isLoading,
    isError,
    refetch,
    isFetching
  } = useDatabaseSchema(database.name, isOpen);

  const handleRefresh = (e: React.MouseEvent) => {
    e.stopPropagation();
    refetch();
  };

  const Chevron = isOpen ? ChevronDown : ChevronRight;

  return (
    <SidebarMenuItem>
      <SidebarMenuButton
        onClick={() => setIsOpen((open) => !open)}
        className='text-sidebar-foreground hover:bg-sidebar-accent'
      >
        <Chevron className='h-4 w-4' />
        {getDatabaseIcon(database.db_type, database.dialect)}
        <span className='flex-1 truncate'>{database.name}</span>

        {isError && !isFetching && (
          <AlertCircle
            className='h-3.5 w-3.5 shrink-0 text-destructive'
            aria-label='Schema fetch failed'
          />
        )}

        <Button
          variant='ghost'
          size='icon'
          onClick={handleRefresh}
          disabled={isFetching}
          tooltip='Refresh Schema'
          aria-label='Refresh Schema'
        >
          <RotateCw className={cn(isFetching && "animate-spin")} />
        </Button>
      </SidebarMenuButton>

      {isOpen && (
        <ConnectionSchemaContent
          databaseName={database.name}
          dialect={database.dialect}
          schema={schema}
          isLoading={isLoading}
          isError={isError}
          onRefresh={handleRefresh}
        />
      )}
    </SidebarMenuItem>
  );
};
