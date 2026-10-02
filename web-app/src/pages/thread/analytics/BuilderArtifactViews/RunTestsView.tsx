import type { ArtifactItem } from "@/hooks/analyticsSteps";
import { MetricKind, type TestResult } from "@/types/eval";
import { parseToolJson } from "../analyticsArtifactHelpers";
import Result from "./Result";

type RunTestsSuiteSummary = {
  score?: number | null;
  total_attempted?: number;
  answered?: number;
  errors?: string[];
};

type RunTestsFileResult = {
  test_file?: string;
  result?: {
    overall_score?: number | null;
    suites?: RunTestsSuiteSummary[];
  };
};

function suiteSummaryToTestResult(suite: RunTestsSuiteSummary): TestResult {
  return {
    errors: suite.errors ?? [],
    // No score means the suite was never judged (it errored first). That is no
    // metric at all — a made-up 0 would read as a suite that ran and failed.
    metrics:
      typeof suite.score === "number"
        ? [{ type: MetricKind.Correctness, score: suite.score, records: [] }]
        : []
  };
}

function runTestsFileKey(file: RunTestsFileResult): string {
  return [
    file.test_file ?? "tests",
    file.result?.overall_score ?? "na",
    file.result?.suites?.length ?? 0
  ].join("-");
}

function runTestsSuiteKey(fileKey: string, suite: RunTestsSuiteSummary): string {
  return [
    fileKey,
    suite.score ?? "na",
    suite.total_attempted ?? 0,
    suite.answered ?? 0,
    (suite.errors ?? []).join("|")
  ].join("-");
}

function normalizeRunTestsOutput(
  raw: {
    message?: string;
    test_file?: string;
    result?: RunTestsFileResult["result"];
    tests_run?: number;
    results?: RunTestsFileResult[];
  } | null
): {
  message?: string;
  files: RunTestsFileResult[];
  testsRun: number;
} {
  if (!raw) {
    return { files: [], testsRun: 0 };
  }

  if (Array.isArray(raw.results)) {
    return {
      message: raw.message,
      files: raw.results,
      testsRun: raw.tests_run ?? raw.results.length
    };
  }

  if (raw.test_file || raw.result) {
    return {
      message: raw.message,
      files: [{ test_file: raw.test_file, result: raw.result }],
      testsRun: 1
    };
  }

  return {
    message: raw.message,
    files: [],
    testsRun: raw.tests_run ?? 0
  };
}

export const RunTestsView = ({ item }: { item: ArtifactItem }) => {
  const input = parseToolJson<{ file_path?: string }>(item.toolInput);
  const output = parseToolJson<{
    message?: string;
    test_file?: string;
    result?: RunTestsFileResult["result"];
    tests_run?: number;
    results?: RunTestsFileResult[];
  }>(item.toolOutput);

  const normalized = normalizeRunTestsOutput(output);
  const files = normalized.files;
  const suiteCount = files.reduce((count, file) => count + (file.result?.suites?.length ?? 0), 0);
  const overallScores = files
    .map((file) => file.result?.overall_score)
    .filter((score): score is number => typeof score === "number");
  const averageScore =
    overallScores.length > 0
      ? overallScores.reduce((sum, score) => sum + score, 0) / overallScores.length
      : null;

  return (
    <div className='flex h-full min-h-0 flex-col p-4'>
      <div className='flex min-h-0 flex-1 flex-col gap-4'>
        <div className='grid grid-cols-2 gap-2'>
          <div className='rounded border bg-muted/30 px-2.5 py-2'>
            <p className='text-[10px] text-muted-foreground uppercase tracking-wide'>Scope</p>
            <p className='font-medium font-mono text-xs'>
              {input?.file_path ? "Single test file" : "All discovered tests"}
            </p>
          </div>
          <div className='rounded border bg-muted/30 px-2.5 py-2'>
            <p className='text-[10px] text-muted-foreground uppercase tracking-wide'>Files Run</p>
            <p className='font-medium font-mono text-xs'>
              {(normalized.testsRun || files.length).toLocaleString()}
            </p>
          </div>
          {input?.file_path && (
            <div className='col-span-2 rounded border bg-muted/30 px-2.5 py-2'>
              <p className='text-[10px] text-muted-foreground uppercase tracking-wide'>
                Requested File
              </p>
              <p className='break-all font-medium font-mono text-xs'>{input.file_path}</p>
            </div>
          )}
          <div className='rounded border bg-muted/30 px-2.5 py-2'>
            <p className='text-[10px] text-muted-foreground uppercase tracking-wide'>Suites</p>
            <p className='font-medium font-mono text-xs'>{suiteCount.toLocaleString()}</p>
          </div>
          {averageScore !== null && (
            <div className='rounded border bg-muted/30 px-2.5 py-2'>
              <p className='text-[10px] text-muted-foreground uppercase tracking-wide'>Avg Score</p>
              <p className='font-medium font-mono text-xs'>{averageScore.toFixed(2)}</p>
            </div>
          )}
        </div>

        {normalized.message && (
          <div className='rounded border bg-muted/20 px-3 py-2'>
            <p className='text-muted-foreground text-xs'>{normalized.message}</p>
          </div>
        )}

        <div className='flex min-h-0 flex-1 flex-col'>
          <p className='mb-1.5 font-medium text-muted-foreground text-xs'>Results</p>
          {files.length > 0 ? (
            <div className='min-h-0 flex-1 space-y-3 overflow-auto rounded border bg-muted/20 p-3'>
              {files.map((file) => {
                const suites = file.result?.suites ?? [];
                const fileScore = file.result?.overall_score;
                const fileKey = runTestsFileKey(file);

                return (
                  <div key={fileKey} className='rounded border bg-background'>
                    <div className='border-b px-3 py-2'>
                      <p className='break-all font-medium font-mono text-xs'>
                        {file.test_file ?? "Discovered tests"}
                      </p>
                      <div className='mt-1 flex flex-wrap gap-3 text-[11px] text-muted-foreground'>
                        {fileScore !== undefined && fileScore !== null && (
                          <span>Overall score: {fileScore.toFixed(2)}</span>
                        )}
                        <span>Suites: {suites.length}</span>
                      </div>
                    </div>

                    <div className='space-y-3 p-3'>
                      {suites.map((suite, suiteIndex) => {
                        const result = suiteSummaryToTestResult(suite);
                        const suiteKey = runTestsSuiteKey(fileKey, suite);

                        return (
                          <div key={suiteKey} className='rounded border'>
                            <div className='border-b bg-muted/20 px-3 py-2 text-[11px] text-muted-foreground'>
                              <span>Suite {suiteIndex + 1}</span>
                              {suite.total_attempted !== undefined && (
                                <span className='ml-3'>Attempts: {suite.total_attempted}</span>
                              )}
                              {suite.answered !== undefined && (
                                <span className='ml-3'>Answered: {suite.answered}</span>
                              )}
                            </div>
                            <div className='px-3 py-2'>
                              <Result result={result} />
                              {result.errors.length > 0 && (
                                <div className='mt-2 space-y-1'>
                                  {result.errors.map((error) => (
                                    <p
                                      key={`${suiteKey}-${error}`}
                                      className='rounded border border-destructive/30 bg-destructive/5 px-2.5 py-1.5 text-[11px] text-destructive'
                                    >
                                      {error}
                                    </p>
                                  ))}
                                </div>
                              )}
                            </div>
                          </div>
                        );
                      })}

                      {suites.length === 0 && (
                        <p className='text-muted-foreground text-xs'>No suite summary returned.</p>
                      )}
                    </div>
                  </div>
                );
              })}
            </div>
          ) : (
            <div className='rounded border bg-muted/20 px-3 py-2'>
              <p className='text-muted-foreground text-xs'>No test results returned.</p>
            </div>
          )}
        </div>
      </div>
    </div>
  );
};
