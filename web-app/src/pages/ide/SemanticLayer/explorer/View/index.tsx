import ErrorAlert from "@/components/ui/ErrorAlert";
import SemanticQueryPanel from "../components/SemanticQueryPanel";
import { useViewExplorerContext } from "./contexts/ViewExplorerContext";
import FieldsSelectionPanel from "./FieldsSelectionPanel";

const ViewExplorer = () => {
  const { viewData, viewError, viewLoading } = useViewExplorerContext();

  if (viewError) {
    return (
      <div className='flex h-full flex-1 flex-col p-4'>
        <ErrorAlert title='Error loading view' message={viewError?.message} />
      </div>
    );
  }

  if (viewLoading || !viewData) {
    return (
      <div className='flex h-full flex-1 flex-col items-center justify-center'>
        <div className='text-muted-foreground'>Loading view data...</div>
      </div>
    );
  }

  return (
    <div className='flex min-h-0 flex-1 flex-col'>
      <div className='flex min-h-0 flex-1'>
        <FieldsSelectionPanel />
        <SemanticQueryPanel />
      </div>
    </div>
  );
};

export { ViewExplorer };
