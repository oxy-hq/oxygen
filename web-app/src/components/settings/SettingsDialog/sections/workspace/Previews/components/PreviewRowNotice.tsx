import { Plus, TriangleAlert } from "lucide-react";
import { Button } from "@/components/ui/shadcn/button";
import { TableCell, TableRow } from "@/components/ui/shadcn/table";
import { cannotCompileMessage, isPreviewNotFound } from "@/libs/utils/preview";

/**
 * What the last create or refresh of this row came back with, when it is
 * something the person has to act on — said in the row, where they asked:
 *
 *  - `cannot_compile`: the server's own words (uncommitted edits in the
 *    branch's worktree, or no checkout), which say what to do;
 *  - `preview_not_found`: the branch was never previewed (or was deleted
 *    since the list loaded), so refreshing it is meaningless — offer Create.
 */
export function rowNoticeFor(
  refreshError: unknown,
  createError: unknown
): { kind: "cannot_compile"; message: string } | { kind: "not_found" } | null {
  const message = cannotCompileMessage(createError) ?? cannotCompileMessage(refreshError);
  if (message) return { kind: "cannot_compile", message };
  if (isPreviewNotFound(refreshError)) return { kind: "not_found" };
  return null;
}

interface Props {
  testId: string;
  notice: NonNullable<ReturnType<typeof rowNoticeFor>>;
  onCreate: () => void;
  isCreating: boolean;
}

export default function PreviewRowNotice({ testId, notice, onCreate, isCreating }: Props) {
  return (
    <TableRow data-testid={`${testId}-notice`}>
      <TableCell colSpan={6}>
        <div role='alert' className='flex flex-wrap items-center gap-2 text-sm'>
          <TriangleAlert className='size-4 shrink-0 text-destructive' aria-hidden />
          {notice.kind === "cannot_compile" ? (
            <span className='text-destructive'>{notice.message}</span>
          ) : (
            <>
              <span>This branch hasn't been previewed yet — create it.</span>
              <Button
                size='sm'
                variant='outline'
                className='h-7'
                onClick={onCreate}
                disabled={isCreating}
                data-testid={`${testId}-create`}
              >
                <Plus />
                {isCreating ? "Creating…" : "Create preview"}
              </Button>
            </>
          )}
        </div>
      </TableCell>
    </TableRow>
  );
}
