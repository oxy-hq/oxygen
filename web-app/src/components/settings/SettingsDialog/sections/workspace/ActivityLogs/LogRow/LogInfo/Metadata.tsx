import { Bot, Database } from "lucide-react";
import { Badge } from "@/components/ui/shadcn/badge";
import { objectNameFromPath } from "@/libs/utils/objectName";
import type { QueryItem } from "@/types/logs";

const Metadata = ({ queryItem }: { queryItem: QueryItem }) => {
  const database = queryItem.database as string | undefined;
  const isVerified = queryItem.is_verified as boolean | undefined;
  const source = queryItem.source ?? "";

  return (
    <div className='flex items-center justify-between'>
      <div className='flex items-center gap-2'>
        {database && (
          <Badge>
            <Database className='mr-1 h-4 w-4' /> {database}
          </Badge>
        )}

        {source && (
          <Badge variant='outline'>
            <Bot className='mr-1 h-4 w-4' /> {objectNameFromPath(source.toString())}
          </Badge>
        )}
      </div>

      {typeof isVerified === "boolean" && (
        <Badge variant={isVerified ? "default" : "destructive"}>
          {isVerified ? "✓ Verified" : "✗ Unverified"}
        </Badge>
      )}
    </div>
  );
};

export default Metadata;
