import { ChevronRight } from "lucide-react";
import { useState } from "react";
import ReactMarkdown from "react-markdown";
import {
  MetricKind,
  type RecallMetric,
  type RecallRecord,
  type Record,
  type SimilarityMetric,
  type TestResult
} from "@/types/eval";

const Similarity = ({ score, records }: SimilarityMetric) => {
  const inconsistencies = records.filter((record) => record.score < 1.0);
  const haveInconsistencies = inconsistencies.length > 0;
  const [showResult, setShowResult] = useState(false);

  return (
    <>
      <div
        className='flex cursor-pointer items-center gap-2 py-1'
        onClick={() => {
          if (haveInconsistencies) {
            setShowResult(!showResult);
          }
        }}
      >
        <ChevronRight
          className={`h-4 w-4 transition-transform duration-300 ease-in-out ${
            showResult ? "rotate-90" : "rotate-0"
          }`}
        />
        <p className='text-sidebar-foreground'>Accuracy score:</p>
        <p className='text-success'>{score}</p>
      </div>

      {haveInconsistencies && (
        <div
          className={`overflow-hidden pl-8 transition-all duration-300 ease-in-out ${
            showResult ? "max-h-[5000px] opacity-100" : "max-h-0 opacity-0"
          }`}
        >
          {inconsistencies.map((record) => (
            <div key={record.cot} className='py-2 font-mono text-muted-foreground text-sm'>
              <p className='py-2'>Inconsistencies were found</p>

              <ReactMarkdown
                components={{
                  code: ({ children, ...props }) => {
                    // A fenced block reaches here as its one text node. Anything else
                    // (nested elements) has no diff text to split, so render nothing
                    // rather than "[object Object]".
                    const text = typeof children === "string" ? children : "";
                    // Split on +++ / --- diff markers while preserving them, then
                    // render each segment as a React text node so injected HTML
                    // in record.cot (LLM-generated) cannot escape into the DOM.
                    const parts = text.split(/(\+\+\+|---)/g);
                    return (
                      <code {...props} className='whitespace-pre-wrap break-words'>
                        {parts.map((part, i) => {
                          const k = `${i}:${part}`;
                          if (part === "+++") {
                            return (
                              <span key={k} className='text-success'>
                                {part}
                              </span>
                            );
                          }
                          if (part === "---") {
                            return (
                              <span key={k} className='text-destructive'>
                                {part}
                              </span>
                            );
                          }
                          return <span key={k}>{part}</span>;
                        })}
                      </code>
                    );
                  },
                  pre: ({ ...props }) => (
                    <pre {...props} className='overflow-x-auto whitespace-pre-wrap break-words' />
                  )
                }}
              >
                {`\`\`\`\n${record.cot}\n\`\`\``}
              </ReactMarkdown>
            </div>
          ))}
        </div>
      )}
    </>
  );
};

const Recall = ({ score, records }: RecallMetric) => {
  const inconsistencies = records.filter((record) => record.score < 1.0);
  const haveInconsistencies = inconsistencies.length > 0;
  const [showResult, setShowResult] = useState(false);
  return (
    <>
      <div
        className='flex cursor-pointer items-center gap-2 py-1'
        onClick={() => {
          if (haveInconsistencies) {
            setShowResult(!showResult);
          }
        }}
      >
        <ChevronRight
          className={`h-4 w-4 transition-transform duration-300 ease-in-out ${
            showResult ? "rotate-90" : "rotate-0"
          }`}
        />
        <p className='text-sidebar-foreground'>Recall score:</p>
        <p className='text-success'>{score}</p>
      </div>

      {haveInconsistencies && (
        <div
          className={`overflow-hidden pl-8 transition-all duration-300 ease-in-out ${
            showResult ? "max-h-[5000px] opacity-100" : "max-h-0 opacity-0"
          }`}
        >
          {inconsistencies.map((record) => (
            <div
              key={record.retrieved_contexts.join(",")}
              className='py-2 font-mono text-muted-foreground text-sm'
            >
              <p className='py-2'>Recall Score: {record.score}</p>
              <p className='text-success'>
                Retrieved documents:
                <ul className='list-inside list-disc'>
                  {record.retrieved_contexts.map((doc) => (
                    <li key={doc} className='list-inside list-disc'>
                      {doc}
                    </li>
                  ))}
                </ul>
              </p>
              <p className='text-destructive'>
                Reference documents:
                <ul className='list-inside list-disc'>
                  {record.reference_contexts.map((doc) => (
                    <li key={doc} className='list-inside list-disc'>
                      {doc}
                    </li>
                  ))}
                </ul>
              </p>
            </div>
          ))}
        </div>
      )}
    </>
  );
};

const Result = ({ result }: { result: TestResult }) => {
  const { metrics } = result;

  return (
    <div className='rounded-b-lg'>
      {metrics.map((metric) => {
        switch (metric.type) {
          case MetricKind.Similarity:
            return (
              <Similarity
                key={metric.type}
                type={metric.type}
                score={metric.score}
                records={metric.records as Record[]}
              />
            );
          case MetricKind.Recall:
            return (
              <Recall
                key={metric.type}
                type={metric.type}
                score={metric.score}
                records={metric.records as RecallRecord[]}
              />
            );
          case MetricKind.Correctness:
            // The only kind `RunTestsView` builds (a suite's correctness score, with
            // no judge records to expand) — without this case every suite rendered
            // an empty block.
            return (
              <div key={metric.type} className='flex items-center gap-2 py-1'>
                <p className='text-sidebar-foreground'>Correctness score:</p>
                <p className='text-success'>{metric.score.toFixed(2)}</p>
              </div>
            );
        }
      })}
    </div>
  );
};

export default Result;
