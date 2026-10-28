import { Workflow as Automation } from "lucide-react";
import { useMemo } from "react";
import OutputLogs from "@/components/automation/output/Logs";
import PageHeader from "@/components/PageHeader";
import { Separator } from "@/components/ui/shadcn/separator";
import { useResumeAutomationThread } from "@/hooks/automation/useResumeAutomationThread";
import useAppsFirstExperience from "@/hooks/useAppsFirstExperience";
import { decodeBase64 } from "@/libs/encoding";
import { objectNameFromPath } from "@/libs/utils/objectName";
import useAutomationThreadStore from "@/stores/useAutomationThread";
import type { ThreadItem } from "@/types/chat";
import ProcessingWarning from "../ProcessingWarning";

const AutomationThread = ({
  thread,
  refetchThread,
  hideHeader
}: {
  thread: ThreadItem;
  refetchThread: () => void;
  hideHeader?: boolean;
}) => {
  const { automationThread } = useAutomationThreadStore();

  const { logs, isLoading } = automationThread.get(thread.id) || {
    logs: [],
    isLoading: false
  };

  // Recover from `agentic_runs.thread_id` after a page reload: the
  // zustand store is in-memory, so the live runner's logs are gone after
  // a refresh. The resume hook fetches the latest automation run for this
  // thread and replays its persisted events into the store. No-ops when
  // the store already has logs (active run in progress).
  useResumeAutomationThread(thread.id);

  // `thread.source` is stored as URL-safe base64 of the automation file
  // path (matches the automation run page's URL param). Decode for the
  // header so the user sees the readable path; fall back to the raw
  // value if decoding fails so we never render an empty header.
  // Apps-first: the automation's plain name, never its file path.
  const plainNames = useAppsFirstExperience();
  const sourcePath = useMemo(() => {
    const source = thread?.source;
    if (!source) return "";
    let path: string;
    try {
      path = decodeBase64(source);
    } catch {
      path = source;
    }
    return plainNames ? objectNameFromPath(path) : path;
  }, [thread?.source, plainNames]);

  return (
    <div className='flex h-full flex-col'>
      {!hideHeader && (
        <PageHeader className='items-center border-border border-b-1'>
          <div className='flex h-full flex-1 items-center justify-center p-2'>
            <div className='flex items-center gap-1 text-muted-foreground'>
              <Automation className='h-4 min-h-4 w-4 min-w-4' />
              <p className='break-all text-sm'>{sourcePath}</p>
            </div>
            <div className='flex h-full items-stretch px-4'>
              <Separator orientation='vertical' />
            </div>

            <p className='text-base-foreground text-sm'>{thread?.title}</p>
          </div>
        </PageHeader>
      )}

      <div className='w-full flex-1'>
        <div className='px-4'>
          <ProcessingWarning
            className='mx-auto mt-2 w-full max-w-page-content'
            threadId={thread.id}
            isLoading={isLoading}
            onRefresh={refetchThread}
          />
        </div>

        <OutputLogs
          isPending={isLoading}
          logs={logs}
          contentClassName='max-w-page-content mx-auto mt-4'
        />
      </div>
    </div>
  );
};

export default AutomationThread;
