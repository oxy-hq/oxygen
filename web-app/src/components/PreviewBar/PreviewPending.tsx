import { Eye, TriangleAlert } from "lucide-react";
import { Button } from "@/components/ui/shadcn/button";
import { Spinner } from "@/components/ui/shadcn/spinner";
import { usePreviewPin } from "@/contexts/PreviewPinContext";

/**
 * Shown INSTEAD of the workspace while a `?preview=` link is being resolved to
 * its branch, or when no preview carries that revision. The page is withheld
 * rather than rendered: without the label it would fetch as a live page and
 * show live numbers under a preview URL.
 */
export function PreviewPending() {
  const { status, revisionId, exit } = usePreviewPin();
  const unavailable = status === "unavailable";

  return (
    <div className='flex h-full w-full flex-col' data-testid='preview-pending'>
      <div className='flex min-h-9 w-full items-center gap-3 bg-info px-3 py-1 text-info-foreground text-sm'>
        <Eye className='size-4 shrink-0' aria-hidden />
        <p className='min-w-0 flex-1 truncate'>
          <span className='font-semibold'>Preview · real data, read-only</span>
          {revisionId && <span className='ml-2 font-mono opacity-80'>{revisionId}</span>}
        </p>
        {revisionId && (
          <Button size='sm' variant='secondary' className='h-7' onClick={exit}>
            Exit preview
          </Button>
        )}
      </div>
      <div className='flex flex-1 flex-col items-center justify-center gap-3 p-6 text-center'>
        {unavailable ? (
          <>
            <TriangleAlert className='size-6 text-muted-foreground' aria-hidden />
            <p className='font-medium text-sm'>This preview isn't available</p>
            <p className='max-w-md text-muted-foreground text-sm'>
              No preview in this workspace serves that revision. It may have been refreshed or
              deleted — open the branch again from Settings → Previews.
            </p>
          </>
        ) : (
          <>
            <Spinner />
            <p className='text-muted-foreground text-sm'>Opening preview…</p>
          </>
        )}
      </div>
    </div>
  );
}
