import {
  AppWindow,
  Workflow as Automation,
  BookOpen,
  Bot,
  Braces,
  Eye,
  File,
  FileCode,
  Table
} from "lucide-react";
import type { BuilderFileChange } from "@/hooks/useBuilderActivity";
import { cn } from "@/libs/shadcn/utils";
import { detectFileType, FILE_TYPE_CONFIGS, FileType } from "@/utils/fileTypes";

// Mirrors getFileIcon in FileNode.tsx
const getFileIcon = (filePath: string) => {
  const fileType = detectFileType(filePath);
  switch (fileType) {
    case FileType.PROCEDURE:
    case FileType.AUTOMATION:
      return Automation;
    case FileType.ANALYTICS_AGENT:
      return Bot;
    case FileType.APP:
      return AppWindow;
    case FileType.VIEW:
      return Eye;
    case FileType.TOPIC:
      return BookOpen;
    case FileType.SQL:
      return FileCode;
    default:
      if (filePath.toLowerCase().endsWith(".json")) return Braces;
      if (filePath.toLowerCase().endsWith(".csv")) return Table;
      return File;
  }
};

/** Strip the compound extension (e.g. ".view.yml") to get just the stem name. */
const getStemName = (filePath: string): string => {
  const basename = filePath.split("/").pop() ?? filePath;
  for (const config of Object.values(FILE_TYPE_CONFIGS)) {
    for (const ext of config.extensions) {
      if (basename.toLowerCase().endsWith(ext)) {
        return basename.slice(0, basename.length - ext.length);
      }
    }
  }
  const dotIdx = basename.lastIndexOf(".");
  return dotIdx > 0 ? basename.slice(0, dotIdx) : basename;
};

interface AcceptedChangePillsProps {
  changes: BuilderFileChange[];
  onSelect: (change: BuilderFileChange) => void;
  selectedId?: string;
}

const AcceptedChangePills = ({ changes, onSelect, selectedId }: AcceptedChangePillsProps) => {
  if (changes.length === 0) return null;

  // Deduplicate by filePath, keeping the last change per file.
  const seen = new Set<string>();
  const deduped = [...changes]
    .reverse()
    .filter((change) => {
      if (seen.has(change.filePath)) return false;
      seen.add(change.filePath);
      return true;
    })
    .reverse()
    .filter((change) => !change.isDeletion);

  if (deduped.length === 0) return null;

  return (
    <div className='mt-3 flex flex-wrap gap-2'>
      {deduped.map((change) => {
        const Icon = getFileIcon(change.filePath);
        const stemName = getStemName(change.filePath);
        const isSelected = selectedId === change.id;
        return (
          <button
            key={change.id}
            type='button'
            aria-pressed={isSelected}
            onClick={() => onSelect(change)}
            title={change.filePath}
            className={cn(
              "flex items-center gap-1.5 rounded-full border px-3 py-1 text-xs transition-colors",
              "hover:bg-accent hover:text-accent-foreground",
              isSelected
                ? "border-primary bg-primary/10 text-primary"
                : "border-border text-muted-foreground"
            )}
          >
            <Icon className='h-3 w-3 shrink-0' />
            <span>{stemName}</span>
          </button>
        );
      })}
    </div>
  );
};

export default AcceptedChangePills;
