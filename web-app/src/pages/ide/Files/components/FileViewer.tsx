import CodeView from "@/components/Code/CodeView";
import ErrorAlert from "@/components/ui/ErrorAlert";
import { Spinner } from "@/components/ui/shadcn/spinner";
import useRevisionFile from "@/hooks/api/revisionFiles/useRevisionFile";
import { apiErrorMessage } from "../errors";

/** One compiled file, read-only. */
export default function FileViewer({ path }: { path: string }) {
  const file = useRevisionFile(path);

  if (file.isPending) {
    return (
      <div className='flex flex-1 items-center justify-center'>
        <Spinner />
      </div>
    );
  }
  if (file.isError) {
    return (
      <div className='p-4'>
        <ErrorAlert
          title={`Could not open ${path}`}
          message={apiErrorMessage(file.error)}
          actions={
            <button type='button' className='text-sm underline' onClick={() => file.refetch()}>
              Retry
            </button>
          }
        />
      </div>
    );
  }

  const { content, language } = file.data;
  return (
    <div className='flex min-h-0 flex-1 flex-col' data-testid='ide-files-viewer'>
      <div className='flex shrink-0 flex-col gap-0.5 border-b px-4 py-2'>
        <span className='truncate font-mono text-sm'>{path}</span>
        {language === "yaml" && (
          <span className='text-muted-foreground text-xs'>
            The compiled definition, shown as YAML: comments and key order are not the file's.
          </span>
        )}
      </div>
      <div className='min-h-0 flex-1'>
        <CodeView value={content} language={language} />
      </div>
    </div>
  );
}
