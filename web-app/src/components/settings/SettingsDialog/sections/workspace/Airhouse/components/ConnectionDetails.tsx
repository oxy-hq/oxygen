import type React from "react";
import { Card, CardContent, CardFooter, CardHeader, CardTitle } from "@/components/ui/shadcn/card";
import useAppsFirstExperience from "@/hooks/useAppsFirstExperience";
import useFactoryAccess from "@/hooks/useFactoryAccess";
import type { AirhouseConnectionInfo } from "@/services/api";
import { CopyableField } from "./CopyableField";

interface ConnectionDetailsProps {
  connection: AirhouseConnectionInfo;
}

export const ConnectionDetails: React.FC<ConnectionDetailsProps> = ({ connection }) => {
  // The config.yml pointer is for whoever works on the workspace's files: apps-first,
  // only a Factory user is shown it.
  const appsFirst = useAppsFirstExperience();
  const factoryUser = useFactoryAccess();
  const showConfigHint = !appsFirst || factoryUser;
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
      {showConfigHint && (
        <CardFooter>
          <p className='text-muted-foreground text-xs'>
            To let agents use it, add an <code>airhouse_managed</code> database entry named{" "}
            <code>{connection.dbname}</code> to the workspace's <code>config.yml</code> in its
            repository and push it.
          </p>
        </CardFooter>
      )}
    </Card>
  );
};
