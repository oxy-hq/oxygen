import ErrorAlert from "@/components/ui/ErrorAlert";
import SemanticQueryPanel from "../components/SemanticQueryPanel";
import { useTopicExplorerContext } from "./contexts/TopicExplorerContext";
import FieldsSelectionPanel from "./FieldsSelectionPanel";

const TopicExplorer = () => {
  const { loadingTopicError, topicLoading, topicData } = useTopicExplorerContext();

  if (loadingTopicError) {
    return (
      <div className='flex h-full flex-1 flex-col items-center justify-center p-4'>
        <ErrorAlert title='Error loading topic' message={loadingTopicError} className='max-w-2xl' />
      </div>
    );
  }

  if (topicLoading || !topicData) {
    return (
      <div className='flex h-full flex-1 flex-col items-center justify-center'>
        <div className='text-muted-foreground'>Loading topic data...</div>
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

export { TopicExplorer };
