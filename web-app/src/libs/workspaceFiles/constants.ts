import { FileType } from "@/utils/fileTypes";

// Reuse a single collator for faster, locale-aware, case-insensitive comparisons
export const NAME_COLLATOR = new Intl.Collator(undefined, {
  sensitivity: "base",
  numeric: true
});

export const IGNORE_FILES_REGEX = [/^docker-entrypoints/, /^output/, /^\./];

// Object file types that should appear in Objects view
export const OBJECT_FILE_TYPES = [
  FileType.PROCEDURE,
  FileType.AUTOMATION,
  FileType.ANALYTICS_AGENT,
  FileType.PIPELINE,
  FileType.VIEW,
  FileType.TOPIC,
  FileType.TEST
];

export enum FilesSubViewMode {
  OBJECTS = "objects",
  FILES = "files"
}
