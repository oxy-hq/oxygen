import { ErrorBoundary } from "react-error-boundary";
import CodeView from "@/components/Code/CodeView";
import SqlResultsTable from "@/components/sql/SqlResultsTable";
import ErrorAlert from "@/components/ui/ErrorAlert";
import type { OmniQueryArtifact } from "@/types/artifact";

type Props = {
  artifact: OmniQueryArtifact;
};

const getCleanOmniObject = (value: OmniQueryArtifact["content"]["value"]) => {
  // Only filter out execution-related fields, keep all omni parameters
  /* eslint-disable @typescript-eslint/no-unused-vars */
  /* eslint-disable sonarjs/no-unused-vars */
  const {
    result: _,
    sql: _sql_query,
    is_result_truncated: _is_result_truncated,
    ...omniParams
  } = value;
  /* eslint-enable @typescript-eslint/no-unused-vars */
  /* eslint-enable sonarjs/no-unused-vars */
  return omniParams;
};

const OmniQueryArtifactPanel = ({ artifact }: Props) => {
  return (
    <div className='flex h-full flex-col'>
      {/* Omni Query JSON Section */}
      <div className='border-b p-4'>
        <h4 className='mb-2 font-medium text-sm'>Omni Query</h4>
        <ErrorBoundary
          resetKeys={[artifact.id]}
          fallback={
            <ErrorAlert
              title='Failed to render Omni query'
              message='The Omni query editor could not be displayed.'
            />
          }
        >
          <div className='h-50 overflow-hidden rounded border'>
            <CodeView
              value={JSON.stringify(getCleanOmniObject(artifact.content.value), null, 2)}
              language='json'
            />
          </div>
        </ErrorBoundary>
      </div>

      <h4 className='mb-2 font-medium text-sm'>Generated SQL</h4>
      {/* Generated SQL Section */}
      <div className='flex-1'>
        <ErrorBoundary
          resetKeys={[artifact.id, artifact.content.value.sql]}
          fallback={
            <ErrorAlert
              className='m-3'
              title='Failed to render SQL editor'
              message='The SQL editor could not be displayed.'
            />
          }
        >
          <CodeView value={artifact.content.value.sql} language='sql' />
        </ErrorBoundary>
      </div>

      {/* Results Section */}
      <div className='flex-1 overflow-auto'>
        {(artifact.content.value.result || artifact.content.value.result_file) && (
          <SqlResultsTable
            result={artifact.content.value.result}
            resultFile={artifact.content.value.result_file}
          />
        )}
      </div>
    </div>
  );
};

export default OmniQueryArtifactPanel;
