import { DatabaseZap } from "lucide-react";
import type React from "react";
import { Button } from "@/components/ui/shadcn/button";
import { Card, CardContent, CardFooter, CardHeader, CardTitle } from "@/components/ui/shadcn/card";
import useAppsFirstExperience from "@/hooks/useAppsFirstExperience";
import useFactoryAccess from "@/hooks/useFactoryAccess";
import type { AirhouseConnectionInfo } from "@/services/api";
import { CopyableField } from "./CopyableField";

interface ConnectionDetailsProps {
  connection: AirhouseConnectionInfo;
  onAddToConfig: (name: string) => void;
  isAddingToConfig: boolean;
}

export const ConnectionDetails: React.FC<ConnectionDetailsProps> = ({
  connection,
  onAddToConfig,
  isAddingToConfig
}) => {
  const appsFirst = useAppsFirstExperience();
  const factoryUser = useFactoryAccess();
  const showConfigAction = !appsFirst || factoryUser;
  return (
    <Card>
      <CardHeader>
        <CardTitle>Connection details</CardTitle>
        <p className='text-muted-foreground text-sm'>
          Use these fields to connect any Postgres-compatible client to your Airhouse database.
        </p>
      </CardHeader>
      <CardContent className='grid gap-4 sm:grid-cols-2'>
        <CopyableField label='Host' value={connection.host} />
        <CopyableField label='Port' value={String(connection.port)} />
        <CopyableField label='Database' value={connection.dbname} />
        <CopyableField label='Role' value={connection.role} />
      </CardContent>
      {/* Writing config.yml is the Factory's; apps-first, only a Factory user is
          offered it. */}
      {showConfigAction && (
        <CardFooter className='flex flex-col items-start gap-2'>
          <Button
            variant='outline'
            onClick={() => onAddToConfig(connection.dbname)}
            disabled={isAddingToConfig}
          >
            <DatabaseZap className='h-4 w-4' />
            {isAddingToConfig ? "Adding…" : "Add to config.yml"}
          </Button>
          <p className='text-muted-foreground text-xs'>
            Adds an <code>airhouse_managed</code> database entry to your <code>config.yml</code> so
            agents and the SQL IDE can use it automatically. Commit the file to persist the change
            across deployments.
          </p>
        </CardFooter>
      )}
    </Card>
  );
};
