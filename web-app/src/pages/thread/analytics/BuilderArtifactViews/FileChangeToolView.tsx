import CodeDiffView from "@/components/Code/CodeDiffView";
import type { ArtifactItem } from "@/hooks/analyticsSteps";
import { parseToolJson } from "../analyticsArtifactHelpers";

type ToolRunEvent = {
  type: string;
  data: unknown;
};

type FileChangePayload = {
  type: "file_change" | "write_file" | "edit_file" | "delete_file";
  file_path: string;
  old_content: string;
  new_content: string;
  description: string;
  delete?: boolean;
};

const FILE_CHANGE_TYPES = new Set(["file_change", "write_file", "edit_file", "delete_file"]);

function parseFileChangePrompt(prompt: string): FileChangePayload | null {
  try {
    const parsed = JSON.parse(prompt);
    if (parsed !== null && typeof parsed === "object" && FILE_CHANGE_TYPES.has(parsed.type)) {
      const payload = parsed as FileChangePayload;
      if (payload.type === "delete_file") payload.delete = true;
      return payload;
    }
  } catch {
    // not a file-change prompt
  }
  return null;
}

function findFileChangePayload(
  input: {
    file_path?: string;
    description?: string;
    delete?: boolean;
  } | null,
  runEvents: ToolRunEvent[]
): FileChangePayload | null {
  if (!input?.file_path || !input.description) return null;

  return (
    [...runEvents]
      .reverse()
      .filter((event) => event.type === "awaiting_input")
      .flatMap((event) => {
        const questions = (event.data as { questions?: Array<{ prompt: string }> } | undefined)
          ?.questions;
        return questions ?? [];
      })
      .map((question) => parseFileChangePrompt(question.prompt))
      .find(
        (payload) =>
          payload &&
          payload.file_path === input.file_path &&
          payload.description === input.description
      ) ?? null
  );
}

function getFileChangeState(
  input: {
    file_path?: string;
    description?: string;
    delete?: boolean;
  } | null,
  runEvents: ToolRunEvent[]
): "accepted" | "pending" {
  if (!input?.file_path || !input.description || runEvents.length === 0) {
    return "pending";
  }

  const matchingAwaitingIndex = runEvents.findIndex((event) => {
    if (event.type !== "awaiting_input") return false;

    const questions = (event.data as { questions?: Array<{ prompt: string }> } | undefined)
      ?.questions;
    return (questions ?? [])
      .map((question) => parseFileChangePrompt(question.prompt))
      .some(
        (payload) =>
          payload &&
          payload.file_path === input.file_path &&
          payload.description === input.description
      );
  });

  if (matchingAwaitingIndex === -1) {
    return "pending";
  }

  const resolved = runEvents
    .slice(matchingAwaitingIndex + 1)
    .some((event) => event.type === "human_input_resolved");

  return resolved ? "accepted" : "pending";
}

const DIFF_CONTEXT_LINES = 5;

function computeDiffSnippet(
  oldContent: string,
  newContent: string
): {
  oldSnippet: string;
  newSnippet: string;
  startLine: number;
  endLine: number;
  totalLines: number;
} {
  const oldLines = oldContent.split("\n");
  const newLines = newContent.split("\n");
  const maxLen = Math.max(oldLines.length, newLines.length);

  let firstChange = -1;
  let lastChange = -1;
  for (let i = 0; i < maxLen; i++) {
    if (oldLines[i] !== newLines[i]) {
      if (firstChange === -1) firstChange = i;
      lastChange = i;
    }
  }

  if (firstChange === -1) {
    return {
      oldSnippet: oldContent,
      newSnippet: newContent,
      startLine: 1,
      endLine: oldLines.length,
      totalLines: oldLines.length
    };
  }

  const start = Math.max(0, firstChange - DIFF_CONTEXT_LINES);
  const end = Math.min(oldLines.length, lastChange + DIFF_CONTEXT_LINES + 1);
  return {
    oldSnippet: oldLines.slice(start, end).join("\n"),
    newSnippet: newLines.slice(start, end).join("\n"),
    startLine: start + 1,
    endLine: end,
    totalLines: oldLines.length
  };
}

function extractToolErrorMessage(toolOutput: string | undefined): string | null {
  if (!toolOutput) return null;
  try {
    const parsed = JSON.parse(toolOutput);
    if (typeof parsed === "string") return parsed;
  } catch {
    // not JSON
  }
  return null;
}

export const FileChangeToolView = ({
  item,
  runEvents = []
}: {
  item: ArtifactItem;
  runEvents?: ToolRunEvent[];
}) => {
  const input = parseToolJson<{
    file_path?: string;
    description?: string;
    delete?: boolean;
    old_string?: string;
    new_string?: string;
  }>(item.toolInput);

  const payload = findFileChangePayload(input, runEvents);
  const filePath = payload?.file_path ?? input?.file_path ?? "?";
  const description = payload?.description ?? input?.description ?? "";
  const isDelete = payload?.delete ?? input?.delete ?? false;
  const oldContent = payload?.old_content ?? input?.old_string ?? "";
  const newContent = payload?.new_content ?? input?.new_string ?? "";
  const action = isDelete ? "Delete" : oldContent ? "Update" : "Create";
  const isUpdate = !isDelete && !!oldContent && !!newContent;
  const { oldSnippet, newSnippet, startLine, endLine, totalLines } = isUpdate
    ? computeDiffSnippet(oldContent, newContent)
    : {
        oldSnippet: oldContent,
        newSnippet: newContent,
        startLine: 1,
        endLine: oldContent.split("\n").length,
        totalLines: oldContent.split("\n").length
      };
  const changeState = getFileChangeState(input, runEvents);
  const toolError = item.isError ? extractToolErrorMessage(item.toolOutput) : null;
  const stateLabel = toolError
    ? "Error"
    : changeState === "accepted"
      ? "Accepted"
      : "Awaiting confirmation";

  return (
    <div className='flex h-full min-h-0 flex-col p-4'>
      <div className='flex min-h-0 flex-1 flex-col gap-4'>
        <div className='grid grid-cols-2 gap-2'>
          <div className='col-span-2 rounded border bg-muted/30 px-2.5 py-2'>
            <p className='text-[10px] text-muted-foreground uppercase tracking-wide'>Path</p>
            <p className='break-all font-medium font-mono text-xs'>{filePath}</p>
          </div>
          <div className='rounded border bg-muted/30 px-2.5 py-2'>
            <p className='text-[10px] text-muted-foreground uppercase tracking-wide'>Action</p>
            <p className='font-medium font-mono text-xs'>{action}</p>
          </div>
          <div className='rounded border bg-muted/30 px-2.5 py-2'>
            <p className='text-[10px] text-muted-foreground uppercase tracking-wide'>State</p>
            <p className='font-medium font-mono text-xs'>{stateLabel}</p>
          </div>
        </div>

        {description && (
          <div>
            <p className='mb-1.5 font-medium text-muted-foreground text-xs'>Description</p>
            <p className='text-xs'>{description}</p>
          </div>
        )}

        <div className='flex min-h-0 flex-1 flex-col gap-2'>
          <div className='flex items-baseline gap-2'>
            <p className='font-medium text-muted-foreground text-xs'>Diff</p>
            {isUpdate && totalLines > 0 && (
              <p className='text-muted-foreground text-xs'>
                lines {startLine}–{endLine} of {totalLines}
              </p>
            )}
          </div>
          {toolError && (
            <div className='rounded-md border border-destructive/50 bg-destructive/5 p-3'>
              <p className='text-destructive text-xs'>Edit failed</p>
            </div>
          )}
          {filePath !== "?" ? (
            <div className='min-h-0 flex-1 overflow-hidden rounded-md border border-border'>
              <CodeDiffView
                original={oldSnippet}
                modified={isDelete ? "" : newSnippet}
                startLine={isUpdate && startLine > 1 ? startLine : 1}
              />
            </div>
          ) : !toolError ? (
            <p className='text-muted-foreground text-xs'>No diff available.</p>
          ) : null}
        </div>
      </div>
    </div>
  );
};
