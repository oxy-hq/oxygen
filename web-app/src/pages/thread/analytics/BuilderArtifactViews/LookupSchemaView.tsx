import CodeView from "@/components/Code/CodeView";
import type { ArtifactItem } from "@/hooks/analyticsSteps";
import { parseToolJson } from "../analyticsArtifactHelpers";

export const LookupSchemaView = ({ item }: { item: ArtifactItem }) => {
  const input = parseToolJson<{ object_name?: string }>(item.toolInput);
  const output = parseToolJson<{ object_name?: string; schema?: unknown }>(item.toolOutput);
  const objectName = output?.object_name ?? input?.object_name ?? "?";
  const schema = output?.schema ? JSON.stringify(output.schema, null, 2) : "";

  return (
    <div className='flex h-full min-h-0 flex-col p-4'>
      <div className='flex min-h-0 flex-1 flex-col gap-4'>
        <div className='rounded border bg-muted/30 px-2.5 py-2'>
          <p className='text-[10px] text-muted-foreground uppercase tracking-wide'>Object</p>
          <p className='break-all font-medium font-mono text-xs'>{objectName}</p>
        </div>

        <div className='flex min-h-0 flex-1 flex-col'>
          <p className='mb-1.5 font-medium text-muted-foreground text-xs'>Schema</p>
          {schema ? (
            <div className='min-h-0 flex-1 overflow-hidden rounded border'>
              <CodeView value={schema} language='json' />
            </div>
          ) : (
            <div className='rounded border bg-muted/20 px-3 py-2'>
              <p className='text-muted-foreground text-xs'>No schema returned.</p>
            </div>
          )}
        </div>
      </div>
    </div>
  );
};
