import { Trash2 } from "lucide-react";
import { useState } from "react";
import { FormProvider, useFieldArray, useForm } from "react-hook-form";
import { toast } from "sonner";
import {
  BigQueryIcon,
  ClickHouseIcon,
  DuckDBIcon,
  MysqlIcon,
  PostgresIcon,
  RedshiftIcon,
  SnowflakeIcon
} from "@/components/icons";
import { Button } from "@/components/ui/shadcn/button";
import { Input } from "@/components/ui/shadcn/input";
import { Spinner } from "@/components/ui/shadcn/spinner";
import { useAuth } from "@/contexts/AuthContext";
import { useCreateDatabaseConfig } from "@/hooks/api/databases/useCreateDatabaseConfig";
import { useTestDatabaseConnection } from "@/hooks/api/databases/useTestDatabaseConnection";
import { offeredDatabaseTypes } from "@/libs/utils/cloudConnections";
import { cn } from "@/libs/utils/cn";
import BigQueryForm from "@/pages/create-workspace/steps/WarehouseStep/BigQueryForm";
import ClickHouseForm from "@/pages/create-workspace/steps/WarehouseStep/ClickHouseForm";
import DuckDBForm from "@/pages/create-workspace/steps/WarehouseStep/DuckDBForm";
import MysqlForm from "@/pages/create-workspace/steps/WarehouseStep/MysqlForm";
import PostgresForm from "@/pages/create-workspace/steps/WarehouseStep/PostgresForm";
import RedshiftForm from "@/pages/create-workspace/steps/WarehouseStep/RedshiftForm";
import SnowflakeForm from "@/pages/create-workspace/steps/WarehouseStep/SnowflakeForm";
import type { DatabaseConfigType, WarehouseConfig, WarehousesFormData } from "@/types/database";
import { TestConnectionSection } from "./TestConnectionSection";

interface WarehouseOption {
  type: DatabaseConfigType;
  label: string;
  icon?: React.ReactNode;
}

/** File DuckDB: local mode only, and deprecated there — Airhouse replaces it. */
const DEPRECATED_TYPES: ReadonlySet<DatabaseConfigType> = new Set(["duckdb"]);

const warehouseOptions: WarehouseOption[] = [
  {
    type: "bigquery",
    label: "BigQuery",
    icon: <BigQueryIcon />
  },
  {
    type: "snowflake",
    label: "Snowflake",
    icon: <SnowflakeIcon />
  },
  {
    type: "postgres",
    label: "PostgreSQL",
    icon: <PostgresIcon />
  },
  {
    type: "redshift",
    label: "Redshift",
    icon: <RedshiftIcon />
  },
  {
    type: "mysql",
    label: "MySQL",
    icon: <MysqlIcon />
  },
  {
    type: "duckdb",
    label: "DuckDB",
    icon: <DuckDBIcon />
  },
  {
    type: "clickhouse",
    label: "ClickHouse",
    icon: <ClickHouseIcon />
  }
];

interface AddDatabaseFormProps {
  onSuccess: () => void;
  onCancel: () => void;
}

export function AddDatabaseForm({ onSuccess, onCancel }: AddDatabaseFormProps) {
  // A cloud deployment offers no connection that resolves against a disk: no
  // file DuckDB, and key fields that name a secret rather than a file.
  const { isLocalMode } = useAuth();
  const options = offeredDatabaseTypes(warehouseOptions, isLocalMode);
  const createMutation = useCreateDatabaseConfig();
  const testConnection = useTestDatabaseConnection();

  const [currentTestingIndex, setCurrentTestingIndex] = useState<number | null>(null);
  // Track which warehouse's results are currently being displayed
  const [displayResultsForIndex, setDisplayResultsForIndex] = useState<number | null>(null);

  const methods = useForm<WarehousesFormData>({
    defaultValues: {
      warehouses: [
        {
          type: "bigquery",
          name: "BIGQUERY_1",
          config: {}
        }
      ]
    }
  });

  const { register, control, handleSubmit, watch, setValue, reset } = methods;

  const { fields, remove } = useFieldArray({
    control,
    name: "warehouses"
  });

  const handleTypeChange = (index: number, value: DatabaseConfigType) => {
    setValue(`warehouses.${index}.type`, value);
    setValue(`warehouses.${index}.config`, {});
  };

  const handleTestConnection = async (index: number) => {
    const warehouse = watch(`warehouses.${index}`) as WarehouseConfig;

    if (!warehouse.config || Object.keys(warehouse.config).length === 0) {
      toast.error("Please fill in the connection details first");
      return;
    }

    setCurrentTestingIndex(index);
    setDisplayResultsForIndex(index);
    testConnection.reset();

    try {
      await testConnection.testConnection({ warehouse });

      // Check if the test completed successfully
      // The hook will update its state, we'll check it in the render
    } catch (error) {
      toast.error("Connection test failed", {
        description: error instanceof Error ? error.message : "Unknown error"
      });
    } finally {
      setCurrentTestingIndex(null);
    }
  };

  const onSubmit = async (data: WarehousesFormData) => {
    try {
      await createMutation.mutateAsync(data);
      reset();
      setDisplayResultsForIndex(null);
      testConnection.reset();
      onSuccess();
    } catch (error) {
      // Error is already handled in the mutation
      console.error("Failed to create database config:", error);
      toast.error("Failed to create database configuration");
    }
  };

  const handleCancel = () => {
    reset();
    setCurrentTestingIndex(null);
    setDisplayResultsForIndex(null);
    testConnection.reset();
    onCancel();
  };

  const handleRemove = (index: number) => {
    remove(index);
  };

  const renderWarehouseForm = (index: number, warehouseType: DatabaseConfigType) => {
    switch (warehouseType) {
      case "bigquery":
        return <BigQueryForm index={index} cloud={!isLocalMode} />;
      case "duckdb":
        return <DuckDBForm index={index} />;
      case "snowflake":
        return <SnowflakeForm index={index} cloud={!isLocalMode} />;
      case "postgres":
        return <PostgresForm index={index} />;
      case "redshift":
        return <RedshiftForm index={index} />;
      case "mysql":
        return <MysqlForm index={index} />;
      case "clickhouse":
        return <ClickHouseForm index={index} />;
      default:
        return null;
    }
  };

  return (
    <div className='space-y-6'>
      <FormProvider {...methods}>
        <form onSubmit={handleSubmit(onSubmit)} className='space-y-6'>
          <div className='space-y-4'>
            {fields.map((field, index) => {
              const warehouseType = watch(`warehouses.${index}.type`) as DatabaseConfigType;

              const defaultName = `${warehouseType.toUpperCase()}_${index + 1}`;
              const nameError = methods.formState.errors?.warehouses?.[index]?.name;

              const isTesting = currentTestingIndex === index && testConnection.isLoading;
              const showTestResult = displayResultsForIndex === index;

              return (
                <div key={field.id} className={cn("flex flex-col gap-4 rounded-md")}>
                  <div>
                    <div className='flex flex-row items-center justify-between gap-4'>
                      <div className='flex flex-1 items-center gap-2'>
                        <Input
                          {...register(`warehouses.${index}.name`, {
                            required: "Connection name is required",
                            validate: {
                              unique: (value) => {
                                const warehouseNames = methods
                                  .getValues("warehouses")
                                  .map((warehouse, i) => (i !== index ? warehouse.name : null));
                                return (
                                  !warehouseNames.includes(value) || "Warehouse name must be unique"
                                );
                              }
                            }
                          })}
                          defaultValue={defaultName}
                          placeholder='Connection Name'
                        />
                      </div>
                      {fields.length > 1 && (
                        <Button
                          type='button'
                          variant='ghost'
                          size='icon'
                          onClick={() => handleRemove(index)}
                          className='h-8 w-8'
                        >
                          <Trash2 className='h-4 w-4' />
                        </Button>
                      )}
                    </div>
                    {nameError && (
                      <p className='mt-1 text-destructive text-xs'>
                        {nameError.message?.toString()}
                      </p>
                    )}
                  </div>

                  <div className='flex flex-wrap gap-4'>
                    {options.map((option) => (
                      <Button
                        key={option.type}
                        type='button'
                        variant='outline'
                        size='icon'
                        aria-label={
                          DEPRECATED_TYPES.has(option.type)
                            ? `${option.label} (deprecated)`
                            : option.label
                        }
                        title={
                          DEPRECATED_TYPES.has(option.type)
                            ? `${option.label} (deprecated)`
                            : option.label
                        }
                        onClick={() => handleTypeChange(index, option.type)}
                        className={cn(
                          "px-8 py-4",
                          warehouseType === option.type && "border-border-strong bg-muted"
                        )}
                      >
                        {option.icon}
                      </Button>
                    ))}
                  </div>

                  {renderWarehouseForm(index, warehouseType)}
                  <TestConnectionSection
                    isTesting={isTesting}
                    showTestResult={showTestResult}
                    testConnection={testConnection}
                    onTest={() => handleTestConnection(index)}
                    disabled={createMutation.isPending}
                  />
                </div>
              );
            })}
          </div>

          <div className='space-y-3'>
            <div className='flex justify-end gap-2'>
              <Button
                type='button'
                variant='outline'
                onClick={handleCancel}
                disabled={createMutation.isPending}
              >
                Cancel
              </Button>
              <Button type='submit' disabled={createMutation.isPending}>
                {createMutation.isPending ? <Spinner /> : "Create"}
              </Button>
            </div>
          </div>
        </form>
      </FormProvider>
    </div>
  );
}
