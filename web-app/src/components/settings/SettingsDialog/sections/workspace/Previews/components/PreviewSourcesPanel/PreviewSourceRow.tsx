import { useState } from "react";
import { Button } from "@/components/ui/shadcn/button";
import { dateAgo, parseUtcTimestamp } from "@/libs/utils/date";
import type { PreviewSourceItem } from "@/types/workspace";
import PreviewSourceForm from "./PreviewSourceForm";

/**
 * One registered sandbox source. Never shows a secret's value — only the var
 * name it points at, same rule as the form that wrote it. Edit swaps the row
 * for the same form used to add one, pre-filled and with the pipeline (the
 * upsert key) locked.
 */
export default function PreviewSourceRow({
  workspaceId,
  source
}: {
  workspaceId: string;
  source: PreviewSourceItem;
}) {
  const [editing, setEditing] = useState(false);
  const updated = parseUtcTimestamp(source.updated_at);
  const testId = `preview-source-${source.pipeline}`;

  if (editing) {
    return (
      <PreviewSourceForm
        workspaceId={workspaceId}
        existing={source}
        onDone={() => setEditing(false)}
      />
    );
  }

  const tokenVar = source.overrides.refresh_token_var ?? source.overrides.access_token_var;

  return (
    <div
      className='flex flex-wrap items-center gap-2 rounded-md border p-2 text-sm'
      data-testid={testId}
    >
      <span className='font-medium font-mono'>{source.pipeline}</span>
      <span className='text-muted-foreground text-xs'>realm {source.overrides.realm_id}</span>
      {tokenVar && <span className='font-mono text-muted-foreground text-xs'>{tokenVar}</span>}
      {updated && (
        <span className='ml-auto text-muted-foreground text-xs' title={updated.toLocaleString()}>
          Updated {dateAgo(updated)}
        </span>
      )}
      <Button
        variant='ghost'
        size='sm'
        onClick={() => setEditing(true)}
        data-testid={`${testId}-edit`}
      >
        Edit
      </Button>
    </div>
  );
}
