import { memo } from "react";
import { FileType } from "@/utils/fileTypes";
import AgenticAnalyticsEditor from "./AgenticAnalytics";
import AirwayEditor from "./Airway";
import AutomationEditor from "./Automation";
import { useEditorContext } from "./contexts/useEditorContext";
import DefaultEditor from "./Default";
import MarkdownEditor from "./Markdown";
import SqlEditor from "./Sql";
import TestFileEditor from "./TestFile";
import TopicEditor from "./Topic";
import ViewEditor from "./View";

const EditorRouterComponent = () => {
  const { fileType } = useEditorContext();

  switch (fileType) {
    case FileType.PROCEDURE:
    case FileType.AUTOMATION:
      return <AutomationEditor />;
    case FileType.ANALYTICS_AGENT:
      return <AgenticAnalyticsEditor />;
    case FileType.PIPELINE:
      return <AirwayEditor />;
    case FileType.SQL:
      return <SqlEditor />;
    case FileType.VIEW:
      return <ViewEditor />;
    case FileType.TOPIC:
      return <TopicEditor />;
    case FileType.MARKDOWN:
      return <MarkdownEditor />;
    case FileType.TEST:
      return <TestFileEditor />;
    default:
      return <DefaultEditor />;
  }
};

export const EditorRouter = memo(EditorRouterComponent);
