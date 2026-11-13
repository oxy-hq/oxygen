import { ErrorBoundary } from "react-error-boundary";
import CodeView from "@/components/Code/CodeView";
import SqlResultsTable from "@/components/sql/SqlResultsTable";
import ErrorAlert from "@/components/ui/ErrorAlert";
import type { SqlArtifact } from "@/types/artifact";

type Props = {
  artifact: SqlArtifact;
};

const SqlArtifactPanel = ({ artifact }: Props) => {
  return (
    <div className='flex h-full flex-col'>
      <div className='flex-1'>
        <ErrorBoundary
          resetKeys={[artifact.id, artifact.content.value.sql_query]}
          fallback={
            <ErrorAlert
              className='m-3'
              title='Failed to render SQL editor'
              message='The SQL editor could not be displayed.'
            />
          }
        >
          <CodeView value={artifact.content.value.sql_query} language='sql' />
        </ErrorBoundary>
      </div>

      {!!artifact.content.value.error && (
        <ErrorAlert
          className='mx-3 my-2 max-h-32 overflow-y-auto'
          message={artifact.content.value.error}
        />
      )}

      {(!artifact.content.value.error || artifact.content.value.result_file) && (
        <div className='flex-1 overflow-auto'>
          <SqlResultsTable
            result={artifact.content.value.result}
            resultFile={artifact.content.value.result_file}
          />
        </div>
      )}
    </div>
  );
};

export default SqlArtifactPanel;
