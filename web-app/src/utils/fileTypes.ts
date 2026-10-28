import { decodeBase64 } from "@/libs/encoding";

export enum FileType {
  PROCEDURE = "procedure",
  AUTOMATION = "automation",
  ANALYTICS_AGENT = "analytics_agent",
  PIPELINE = "pipeline",
  SQL = "sql",
  VIEW = "view",
  TOPIC = "topic",
  TEST = "test",
  MARKDOWN = "markdown",
  DEFAULT = "default"
}

export interface FileTypeConfig {
  type: FileType;
  extensions: string[];
  editorComponent: string;
}

export const FILE_TYPE_CONFIGS: Record<FileType, FileTypeConfig> = {
  [FileType.TEST]: {
    type: FileType.TEST,
    extensions: [".test.yml", ".test.yaml"],
    editorComponent: "TestEditor"
  },
  // Automation is the canonical type (formerly Procedure / Workflow). It is
  // listed first so `.automation.yml` resolves to AUTOMATION; the legacy
  // PROCEDURE type is kept so existing files keep opening.
  [FileType.AUTOMATION]: {
    type: FileType.AUTOMATION,
    extensions: [".automation.yml", ".automation.yaml"],
    editorComponent: "AutomationEditor"
  },
  [FileType.PROCEDURE]: {
    type: FileType.PROCEDURE,
    extensions: [".procedure.yml", ".procedure.yaml"],
    editorComponent: "AutomationEditor"
  },
  [FileType.ANALYTICS_AGENT]: {
    type: FileType.ANALYTICS_AGENT,
    extensions: [".agentic.yml", ".agentic.yaml"],
    editorComponent: "AgenticAnalyticsEditor"
  },
  [FileType.PIPELINE]: {
    type: FileType.PIPELINE,
    extensions: [".airway.yml", ".airway.yaml"],
    editorComponent: "AirwayEditor"
  },
  [FileType.SQL]: {
    type: FileType.SQL,
    extensions: [".sql"],
    editorComponent: "SqlEditor"
  },
  [FileType.VIEW]: {
    type: FileType.VIEW,
    extensions: [".view.yml", ".view.yaml"],
    editorComponent: "ViewEditor"
  },
  [FileType.TOPIC]: {
    type: FileType.TOPIC,
    extensions: [".topic.yml", ".topic.yaml"],
    editorComponent: "TopicEditor"
  },
  [FileType.MARKDOWN]: {
    type: FileType.MARKDOWN,
    extensions: [".md", ".mdx"],
    editorComponent: "MarkdownEditor"
  },
  [FileType.DEFAULT]: {
    type: FileType.DEFAULT,
    extensions: [],
    editorComponent: "DefaultEditor"
  }
};

export const detectFileType = (filePath: string): FileType => {
  const lowerPath = filePath.toLowerCase();

  for (const config of Object.values(FILE_TYPE_CONFIGS)) {
    if (config.extensions.some((ext) => lowerPath.endsWith(ext))) {
      return config.type;
    }
  }

  return FileType.DEFAULT;
};

export const decodeFilePath = (pathb64: string): string => {
  try {
    return decodeBase64(pathb64);
  } catch {
    return "";
  }
};
