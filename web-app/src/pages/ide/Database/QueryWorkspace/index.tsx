import {
  ResizableHandle,
  ResizablePanel,
  ResizablePanelGroup
} from "@/components/ui/shadcn/resizable";
import QueryEditor from "./components/QueryEditor";
import QueryResults from "./components/QueryResults";

/**
 * The SQL editor and its results. A query runs against the workspace's
 * compiled database config (`POST /sql/query`, FleetOk). Tabs live in this
 * browser only: saving a query as a workspace file went with the IDE, so a
 * query worth keeping goes into the repository.
 */
export default function QueryWorkspacePage() {
  return (
    <div className='flex h-full flex-col'>
      <div className='flex-1 overflow-hidden'>
        <ResizablePanelGroup direction='vertical'>
          <ResizablePanel defaultSize={60} minSize={30}>
            <QueryEditor />
          </ResizablePanel>

          <ResizableHandle withHandle />

          <ResizablePanel defaultSize={40} minSize={20}>
            <QueryResults />
          </ResizablePanel>
        </ResizablePanelGroup>
      </div>
    </div>
  );
}
