import {
  AppWindow,
  Workflow as Automation,
  BookOpen,
  Bot,
  Braces,
  Database,
  Eye,
  FileCode,
  ShieldCheck,
  Table
} from "lucide-react";
import type { FileTreeModel } from "@/types/file";
import { detectFileType, FileType } from "@/utils/fileTypes";
import { NAME_COLLATOR, OBJECT_FILE_TYPES } from "./constants";

// Helper to check if a file is an object type
const isObjectFile = (file: FileTreeModel): boolean => {
  if (file.is_dir) return false;
  const fileType = detectFileType(file.path);
  return OBJECT_FILE_TYPES.includes(fileType);
};

// Helper to get clean object name (without extension)
export const getObjectName = (file: FileTreeModel): string => {
  const fileName = file.name;
  return fileName
    .replace(/\.test\.(yml|yaml)$/, "")
    .replace(/\.agentic\.(yml|yaml)$/, "")
    .replace(/\.(procedure|workflow|automation|app|view|topic|airway)\.(yml|yaml)$/, "")
    .replace(/\.(yml|yaml)$/, "");
};

export const getFileTypeIcon = (fileType: FileType, fileName?: string) => {
  switch (fileType) {
    case FileType.PROCEDURE:
    case FileType.AUTOMATION:
      return Automation;
    case FileType.ANALYTICS_AGENT:
      return Bot;
    case FileType.PIPELINE:
      return Database;
    case FileType.APP:
      return AppWindow;
    case FileType.VIEW:
      return Eye;
    case FileType.TOPIC:
      return BookOpen;
    case FileType.TEST:
      return ShieldCheck;
    case FileType.SQL:
      return FileCode;
    default:
      if (fileName?.toLowerCase().endsWith(".json")) {
        return Braces;
      }
      if (fileName?.toLowerCase().endsWith(".csv")) {
        return Table;
      }
      return null;
  }
};

interface GroupedObjects {
  automations: FileTreeModel[];
  pipelines: FileTreeModel[];
  agents: FileTreeModel[];
  apps: FileTreeModel[];
  tests: FileTreeModel[];
  semanticObjects: FileTreeModel[];
}

export const groupObjectsByType = (files: FileTreeModel[]): GroupedObjects => {
  const groups: GroupedObjects = {
    automations: [],
    pipelines: [],
    agents: [],
    apps: [],
    tests: [],
    semanticObjects: []
  };

  files.forEach((file) => {
    if (file.is_dir) return;
    const fileType = detectFileType(file.path);

    switch (fileType) {
      case FileType.PROCEDURE:
      case FileType.AUTOMATION:
        groups.automations.push(file);
        break;
      case FileType.PIPELINE:
        groups.pipelines.push(file);
        break;
      case FileType.ANALYTICS_AGENT:
        groups.agents.push(file);
        break;
      case FileType.APP:
        groups.apps.push(file);
        break;
      case FileType.TEST:
        groups.tests.push(file);
        break;
      case FileType.VIEW:
      case FileType.TOPIC:
        groups.semanticObjects.push(file);
        break;
      // Not objects — absent from OBJECT_FILE_TYPES, so the Objects view never lists
      // them. Named rather than a `default` so a new FileType has to be placed here.
      case FileType.SQL:
      case FileType.MARKDOWN:
      case FileType.DEFAULT:
        break;
    }
  });

  groups.automations.sort((a, b) => NAME_COLLATOR.compare(a.name, b.name));
  groups.pipelines.sort((a, b) => NAME_COLLATOR.compare(a.name, b.name));
  groups.agents.sort((a, b) => NAME_COLLATOR.compare(a.name, b.name));
  groups.apps.sort((a, b) => NAME_COLLATOR.compare(a.name, b.name));
  groups.tests.sort((a, b) => NAME_COLLATOR.compare(a.name, b.name));
  groups.semanticObjects.sort((a, b) => NAME_COLLATOR.compare(a.name, b.name));

  return groups;
};

// Helper to get all object files from the full file list
export const getAllObjectFiles = (allFiles: FileTreeModel[]): FileTreeModel[] => {
  const objectFiles: FileTreeModel[] = [];

  const traverse = (files: FileTreeModel[]) => {
    files.forEach((file) => {
      if (isObjectFile(file)) {
        objectFiles.push(file);
      }
      if (file.is_dir && file.children) {
        traverse(file.children);
      }
    });
  };

  traverse(allFiles);
  return objectFiles;
};
