import { AlertTriangle } from "lucide-react";
import { useParams } from "react-router-dom";
import LoadingSkeleton from "@/components/ui/LoadingSkeleton";
import useThread from "@/hooks/api/threads/useThread";
import AnalyticsThread from "./analytics";
import AutomationThread from "./automation";

const ThreadNotFound = () => (
  <div className='flex h-64 flex-col items-center justify-center p-8 text-center'>
    <AlertTriangle className='mb-4 h-16 w-16 text-warning' />
    <h2 className='mb-2 font-semibold text-2xl text-muted-foreground'>Thread Not Found</h2>
    <p className='max-w-md text-muted-foreground'>
      The thread you're looking for doesn't exist or may have been removed.
    </p>
    <button
      type='button'
      onClick={() => window.history.back()}
      className='mt-6 rounded-md bg-primary px-4 py-2 text-primary-foreground transition-colors hover:bg-primary/90'
    >
      Go Back
    </button>
  </div>
);

export const Thread = ({
  projectId,
  threadId: threadIdProp,
  hideHeader
}: {
  projectId?: string;
  threadId?: string;
  /** Suppress the per-thread page header — used when the thread is embedded
   *  in the Ask dock, which supplies its own header. */
  hideHeader?: boolean;
}) => {
  const { threadId: threadIdParam } = useParams();
  const threadId = threadIdProp ?? threadIdParam;
  const {
    data: thread,
    isPending,
    isSuccess,
    refetch
  } = useThread(threadId ?? "", true, false, false, projectId);

  if (isPending) {
    return <LoadingSkeleton variant='page' />;
  }

  if (!thread) {
    return <ThreadNotFound />;
  }

  if (isSuccess && thread) {
    switch (thread.source_type) {
      case "workflow":
        return (
          <AutomationThread
            thread={thread}
            refetchThread={() => refetch()}
            hideHeader={hideHeader}
          />
        );
      case "analytics":
        return <AnalyticsThread key={thread.id} thread={thread} hideHeader={hideHeader} />;
      default:
        return <ThreadNotFound />;
    }
  }

  return <ThreadNotFound />;
};

const ThreadPage = () => {
  const { threadId, projectId } = useParams();
  return <Thread key={`${projectId}-${threadId}`} projectId={projectId} />;
};

export default ThreadPage;
