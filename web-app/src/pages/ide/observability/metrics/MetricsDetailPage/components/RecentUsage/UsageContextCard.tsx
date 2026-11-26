import { ChevronDown, ChevronRight, Clock, ExternalLink } from "lucide-react";
import { useState } from "react";
import { useNavigate } from "react-router-dom";
import { encodeBase64 } from "@/libs/encoding";
import { cn } from "@/libs/shadcn/utils";
import ROUTES from "@/libs/utils/routes";
import type { RecentUsage } from "@/services/api/metrics";
import useCurrentOrg from "@/stores/useCurrentOrg";
import { CONTEXT_TYPE_CONFIG } from "../../constants";
import { getTimeAgo, parseContextItems } from "../../utils";
import ContextItemDisplay from "./ContextItemDisplay";
import HighlightedText from "./HighlightedText";
import SourceTypeBadge from "./SourceTypeBadge";

interface UsageContextCardProps {
  usage: RecentUsage;
  metricName: string;
  onTraceClick: (traceId: string) => void;
}

export default function UsageContextCard({
  usage,
  metricName,
  onTraceClick
}: UsageContextCardProps) {
  const navigate = useNavigate();
  const orgSlug = useCurrentOrg((s) => s.org?.slug) ?? "";
  const [isExpanded, setIsExpanded] = useState(false);
  const contextItems = parseContextItems(usage.context);
  const hasContext = contextItems.length > 0;

  // Get a preview from the first non-semantic context item
  const previewItem = contextItems.find(
    (item) => item.type !== "semantic" && typeof item.content === "string"
  );
  const previewContent =
    previewItem && typeof previewItem.content === "string" ? previewItem.content : null;
  const previewIsSQL = previewItem?.type === "sql";

  return (
    <div
      className={cn(
        "group relative border transition-all duration-200",
        "hover:border-primary/30",
        isExpanded && "border-primary/30"
      )}
    >
      {/* Main content */}
      <div className='space-y-3 p-4'>
        {/* Header */}
        <div className='flex items-start justify-between gap-3'>
          <div className='flex flex-wrap items-center gap-2'>
            <SourceTypeBadge sourceType={usage.source_type || "agent"} />
            {/* Context type badges */}
            {usage.context_types?.map((ct, idx) => {
              const config = CONTEXT_TYPE_CONFIG[ct];
              if (!config) return null;
              return (
                <span
                  key={idx}
                  className={cn(
                    "inline-flex items-center gap-1.5 rounded-full border px-2.5 py-1 font-medium text-xs",
                    config.bgColor,
                    config.color
                  )}
                >
                  {config.icon}
                  {config.label}
                </span>
              );
            })}
          </div>
          <div className='flex items-center gap-2 text-muted-foreground text-xs'>
            <Clock className='h-3 w-3' />
            <span>{getTimeAgo(usage.created_at)}</span>
          </div>
        </div>

        {/* Source ref */}
        {usage.source_ref && (
          <div className='text-sm'>
            <button
              onClick={() => {
                const pathb64 = encodeBase64(usage.source_ref);
                navigate(ROUTES.ORG(orgSlug).WORKSPACE().IDE.FILES.FILE(pathb64));
              }}
              className='text-left font-mono text-muted-foreground text-xs underline-offset-4 transition-colors hover:text-primary hover:underline'
            >
              {usage.source_ref}
            </button>
          </div>
        )}

        {/* Quick context preview */}
        {previewContent && !isExpanded && (
          <div
            className={cn(
              "border p-2",
              previewIsSQL ? "border-muted bg-muted/50" : "border-muted/50 bg-muted/30"
            )}
          >
            {previewIsSQL ? (
              <code className='line-clamp-2 font-mono text-vis-cyan text-xs'>
                <HighlightedText text={previewContent.slice(0, 100)} highlight={metricName} />
                {previewContent.length > 100 && "..."}
              </code>
            ) : (
              <p className='line-clamp-2 text-muted-foreground text-xs'>
                <HighlightedText text={previewContent.slice(0, 150)} highlight={metricName} />
                {previewContent.length > 150 && "..."}
              </p>
            )}
          </div>
        )}

        {/* Links row */}
        <div className='flex items-center gap-3 text-xs'>
          <button
            onClick={() => onTraceClick(usage.trace_id)}
            className='flex items-center gap-1 text-muted-foreground transition-colors hover:text-primary'
          >
            <ExternalLink className='h-3 w-3' />
            <span className='font-mono'>{usage.trace_id.slice(0, 8)}...</span>
          </button>

          {hasContext && (
            <button
              onClick={() => setIsExpanded(!isExpanded)}
              className='ml-auto flex items-center gap-1 text-muted-foreground transition-colors hover:text-foreground'
            >
              {isExpanded ? (
                <ChevronDown className='h-3 w-3' />
              ) : (
                <ChevronRight className='h-3 w-3' />
              )}
              <span>{isExpanded ? "Less" : `More (${contextItems.length})`}</span>
            </button>
          )}
        </div>
      </div>

      {/* Expanded context */}
      {isExpanded && hasContext && (
        <div className='space-y-4 border-t bg-muted/20 px-4 py-3'>
          {contextItems.map((item, idx) => (
            <ContextItemDisplay key={idx} item={item} metricName={metricName} />
          ))}
        </div>
      )}
    </div>
  );
}
