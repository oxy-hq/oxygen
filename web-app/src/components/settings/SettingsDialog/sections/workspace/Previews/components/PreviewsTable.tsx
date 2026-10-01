import TableContentWrapper from "@/components/settings/components/TableContentWrapper";
import TableWrapper from "@/components/settings/components/TableWrapper";
import { Table, TableBody, TableHead, TableHeader, TableRow } from "@/components/ui/shadcn/table";
import { usePreviews } from "@/hooks/api/workspaces/usePreviews";
import type { WorkspacePreview } from "@/types/workspace";
import PreviewRow from "./PreviewRow";

interface Props {
  workspaceId: string;
  onOpen: (preview: WorkspacePreview) => void;
}

/** Every preview in the workspace. The list polls itself while any row compiles. */
export default function PreviewsTable({ workspaceId, onOpen }: Props) {
  const { data, isLoading, error, refetch } = usePreviews(workspaceId);
  const previews = data ?? [];

  return (
    <TableWrapper>
      <Table>
        <TableHeader>
          <TableRow>
            <TableHead>Branch</TableHead>
            <TableHead>Status</TableHead>
            <TableHead>Checks</TableHead>
            <TableHead>Author</TableHead>
            <TableHead>Last compiled</TableHead>
            <TableHead className='text-right'>Actions</TableHead>
          </TableRow>
        </TableHeader>
        <TableBody>
          <TableContentWrapper
            isEmpty={previews.length === 0}
            loading={isLoading}
            colSpan={6}
            // Only a list that never loaded is an error state. A poll that
            // fails mid-compile keeps the rows it already has on screen.
            error={data ? undefined : error?.message}
            noFoundTitle='No previews yet'
            noFoundDescription='Create one from a branch to open the product on it without making it live.'
            onRetry={refetch}
          >
            {previews.map((preview) => (
              <PreviewRow
                key={preview.branch}
                workspaceId={workspaceId}
                preview={preview}
                onOpen={onOpen}
              />
            ))}
          </TableContentWrapper>
        </TableBody>
      </Table>
    </TableWrapper>
  );
}
