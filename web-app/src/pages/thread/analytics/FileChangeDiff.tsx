import CodeDiffView from "@/components/Code/CodeDiffView";
import Markdown from "@/components/Markdown";
import { Button } from "@/components/ui/shadcn/button";

export interface FileChangePayload {
  type: "file_change" | "write_file" | "edit_file" | "delete_file";
  file_path: string;
  old_content: string;
  new_content: string;
  description: string;
  delete?: boolean;
}

const FILE_CHANGE_TYPES = new Set(["file_change", "write_file", "edit_file", "delete_file"]);

export function parseFileChange(prompt: string): FileChangePayload | null {
  try {
    const parsed = JSON.parse(prompt);
    if (parsed === null || typeof parsed !== "object") return null;
    if (FILE_CHANGE_TYPES.has(parsed.type)) {
      const payload = parsed as FileChangePayload;
      if (payload.type === "delete_file") payload.delete = true;
      return payload;
    }
  } catch {
    // not JSON
  }
  return null;
}

interface FileChangeDiffProps {
  payload: FileChangePayload;
  suggestions: string[];
  onAnswer: (text: string) => void;
  isAnswering: boolean;
}

const FileChangeDiff = ({ payload, suggestions, onAnswer, isAnswering }: FileChangeDiffProps) => {
  return (
    <div className='space-y-3'>
      <div>
        <p className='mb-0.5 text-muted-foreground text-xs'>{payload.file_path}</p>
        <Markdown>{payload.description}</Markdown>
      </div>
      {payload.delete ? (
        <div className='overflow-hidden rounded-md border border-destructive/50 bg-destructive/5'>
          <div className='border-destructive/20 border-b px-3 py-2'>
            <span className='font-medium text-destructive text-xs'>File will be deleted</span>
          </div>
          <div style={{ height: 200 }}>
            <CodeDiffView original={payload.old_content} modified='' />
          </div>
        </div>
      ) : (
        <div className='overflow-hidden rounded-md border border-border' style={{ height: 360 }}>
          <CodeDiffView original={payload.old_content} modified={payload.new_content} />
        </div>
      )}
      {suggestions.length > 0 && (
        <div className='flex flex-wrap gap-2'>
          {suggestions.map((s) => (
            <Button
              key={s}
              data-testid={`file-change-${s.toLowerCase()}`}
              variant={s === "Accept" ? "default" : "outline"}
              size='sm'
              onClick={() => onAnswer(s)}
              disabled={isAnswering}
            >
              {s}
            </Button>
          ))}
        </div>
      )}
    </div>
  );
};

export default FileChangeDiff;
