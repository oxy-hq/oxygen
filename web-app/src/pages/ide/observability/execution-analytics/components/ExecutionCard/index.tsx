import { ChevronDown, ChevronRight, Clock, ExternalLink } from "lucide-react";
import { useState } from "react";
import { useNavigate } from "react-router-dom";
import ErrorAlert from "@/components/ui/ErrorAlert";
import { encodeBase64 } from "@/libs/encoding";
import { cn } from "@/libs/shadcn/utils";
import ROUTES from "@/libs/utils/routes";
import useCurrentOrg from "@/stores/useCurrentOrg";
import { formatTimeAgo } from "../../../utils";
import type { ExecutionDetail } from "../../types";
import DataDisplay from "./DataDisplay";
import ExecutionTypeBadge from "./ExecutionTypeBadge";
import SqlDisplay from "./SqlDisplay";
import StatusBadge from "./StatusBadge";
import VerifiedBadge from "./VerifiedBadge";

interface ExecutionCardProps {
  execution: ExecutionDetail;
}

function formatDuration(ms: number): string {
  if (ms < 1) return "<1ms";
  if (ms < 1000) return `${Math.round(ms)}ms`;
  return `${(ms / 1000).toFixed(1)}s`;
}

export default function ExecutionCard({ execution }: ExecutionCardProps) {
  const navigate = useNavigate();
  const orgSlug = useCurrentOrg((s) => s.org?.slug) ?? "";
  const [isExpanded, setIsExpanded] = useState(false);

  const handleViewTrace = (traceId: string) => {
    navigate(ROUTES.ORG(orgSlug).WORKSPACE().IDE.OBSERVABILITY.TRACE(traceId));
  };

  const isSuccess = execution.status === "success";
  const sql = execution.sql || execution.generatedSql || "";
  const hasSql = !!sql;
  const hasExpandableContent = hasSql || execution.error || execution.output || execution.toolInput;

  return (
    <div
      className={cn(
        "group relative rounded-xl border transition-all duration-200",
        "hover:border-primary/30 hover:shadow-md hover:shadow-primary/5",
        isExpanded && "border-primary/30"
      )}
    >
      {/* Main content */}
      <div className='space-y-3 p-4'>
        {/* Header */}
        <div className='flex items-start justify-between gap-3'>
          <div className='flex flex-wrap items-center gap-2'>
            <ExecutionTypeBadge executionType={execution.executionType} />
            <VerifiedBadge isVerified={execution.isVerified} />
            <StatusBadge isSuccess={isSuccess} />
          </div>
          <div className='flex items-center gap-2 text-muted-foreground text-xs'>
            <Clock className='h-3 w-3' />
            <span>{formatTimeAgo(execution.timestamp)}</span>
          </div>
        </div>

        {execution.sourceRef && (
          <div className='text-sm'>
            <button
              onClick={() => {
                const pathb64 = encodeBase64(execution.sourceRef);
                navigate(ROUTES.ORG(orgSlug).WORKSPACE().IDE.FILES.FILE(pathb64));
              }}
              className='text-left font-mono text-muted-foreground text-xs underline-offset-4 transition-colors hover:text-primary hover:underline'
            >
              {execution.sourceRef}
            </button>
          </div>
        )}

        <div className='flex items-center gap-4 text-muted-foreground text-xs'>
          {execution.topic && (
            <span>
              <span className='text-muted-foreground'>Topic:</span>{" "}
              <span className='font-medium text-foreground'>{execution.topic}</span>
            </span>
          )}
          {execution.database && (
            <span>
              <span className='text-muted-foreground'>Database:</span>{" "}
              <span className='font-medium text-foreground'>{execution.database}</span>
            </span>
          )}
          {execution.automationRef && (
            <span>
              <span className='text-muted-foreground'>Automation ref:</span>{" "}
              <span className='font-medium text-foreground'>
                <button
                  onClick={() => {
                    const pathb64 = encodeBase64(execution.automationRef || "");
                    navigate(ROUTES.ORG(orgSlug).WORKSPACE().IDE.FILES.FILE(pathb64));
                  }}
                  className='text-left font-mono text-xs underline-offset-4 transition-colors hover:text-primary hover:underline'
                >
                  {execution.automationRef}
                </button>
              </span>
            </span>
          )}
          {execution.agentRef && (
            <span>
              <span className='text-muted-foreground'>Agent ref:</span>{" "}
              <span className='font-medium text-foreground'>
                <button
                  onClick={() => {
                    const pathb64 = encodeBase64(execution.agentRef || "");
                    navigate(ROUTES.ORG(orgSlug).WORKSPACE().IDE.FILES.FILE(pathb64));
                  }}
                  className='text-left font-mono text-xs underline-offset-4 transition-colors hover:text-primary hover:underline'
                >
                  {execution.agentRef}
                </button>
              </span>
            </span>
          )}
          {execution.integration && (
            <span>
              <span className='text-muted-foreground'>Integration:</span>{" "}
              <span className='font-medium text-foreground'>{execution.integration}</span>
            </span>
          )}
          {execution.endpoint && (
            <span>
              <span className='text-muted-foreground'>Endpoint:</span>{" "}
              <span className='font-medium text-foreground'>{execution.endpoint}</span>
            </span>
          )}
          <span>
            <span className='text-muted-foreground'>Duration:</span>{" "}
            <span className='font-medium text-foreground'>
              {formatDuration(execution.durationMs)}
            </span>
          </span>
        </div>

        {/* Quick previews when collapsed */}
        {!isExpanded && hasSql && <SqlDisplay sql={sql} isPreview />}

        <div className='flex items-center gap-3 text-xs'>
          <button
            onClick={() => handleViewTrace(execution.traceId)}
            className='flex items-center gap-1 text-muted-foreground transition-colors hover:text-primary'
          >
            <ExternalLink className='h-3 w-3' />
            <span className='font-mono'>{execution.traceId.slice(0, 8)}...</span>
          </button>

          {hasExpandableContent && (
            <button
              onClick={() => setIsExpanded(!isExpanded)}
              className='ml-auto flex items-center gap-1 text-muted-foreground transition-colors hover:text-foreground'
            >
              {isExpanded ? (
                <ChevronDown className='h-3 w-3' />
              ) : (
                <ChevronRight className='h-3 w-3' />
              )}
              <span>{isExpanded ? "Less" : "More"}</span>
            </button>
          )}
        </div>
      </div>

      {isExpanded && hasExpandableContent && (
        <div className='space-y-4 border-t bg-muted/20 px-4 py-3'>
          {hasSql && (
            <SqlDisplay
              sql={sql}
              label={execution.executionType === "semantic_query" ? "Generated SQL" : "SQL Query"}
            />
          )}

          {execution.semanticQueryParams && (
            <DataDisplay value={execution.semanticQueryParams} label='Semantic Query Parameters' />
          )}

          {execution.toolInput && <DataDisplay value={execution.toolInput} label='Input' />}

          {execution.error && <ErrorAlert title='Execution Error' message={execution.error} />}

          {execution.output && <DataDisplay value={execution.output} label='Output' />}
        </div>
      )}
    </div>
  );
}
