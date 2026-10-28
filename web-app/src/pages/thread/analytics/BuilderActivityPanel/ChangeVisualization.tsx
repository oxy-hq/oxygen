import type { BuilderFileChange } from "@/hooks/useBuilderActivity";
import { AutomationGraph } from "./AutomationGraph";
import { AwGraph } from "./AwGraph";
import { GenericFileDiff } from "./GenericFileDiff";
import { SemanticTopicGraph } from "./SemanticTopicGraph";
import { SemanticViewGraph } from "./SemanticViewGraph";
import { TestGraph } from "./TestGraph";
import { tryParseAutomation, tryParseAw, tryParseTest, tryParseTopic, tryParseView } from "./types";

interface ChangeVisualizationProps {
  change: BuilderFileChange;
}

const ChangeVisualization = ({ change }: ChangeVisualizationProps) => {
  const p = change.filePath;

  const isViewFile = p.endsWith(".view.yml") || p.endsWith(".view.yaml");
  const isAutomationFile =
    p.endsWith(".procedure.yml") ||
    p.endsWith(".procedure.yaml") ||
    p.endsWith(".automation.yml") ||
    p.endsWith(".automation.yaml");
  const isTopicFile = p.endsWith(".topic.yml") || p.endsWith(".topic.yaml");
  const isAwFile = p.endsWith(".agentic.yml") || p.endsWith(".agentic.yaml");
  const isTestFile = p.endsWith(".test.yml") || p.endsWith(".test.yaml");

  const old = change.oldContent || null;

  const newView = isViewFile ? tryParseView(change.newContent) : null;
  const oldView = isViewFile ? tryParseView(old ?? "") : null;

  const newWf = isAutomationFile ? tryParseAutomation(change.newContent) : null;
  const oldWf = isAutomationFile ? tryParseAutomation(old ?? "") : null;

  const newTopic = isTopicFile ? tryParseTopic(change.newContent) : null;
  const oldTopic = isTopicFile ? tryParseTopic(old ?? "") : null;

  const newAw = isAwFile ? tryParseAw(change.newContent) : null;
  const oldAw = isAwFile ? tryParseAw(old ?? "") : null;

  const newTest = isTestFile ? tryParseTest(change.newContent) : null;
  const oldTest = isTestFile ? tryParseTest(old ?? "") : null;

  if (newView) return <SemanticViewGraph change={change} oldView={oldView} newView={newView} />;
  if (newWf) return <AutomationGraph change={change} oldWf={oldWf} newWf={newWf} />;
  if (newTopic)
    return <SemanticTopicGraph change={change} oldTopic={oldTopic} newTopic={newTopic} />;
  if (newAw) return <AwGraph change={change} oldAw={oldAw} newAw={newAw} />;
  if (newTest) return <TestGraph change={change} oldTest={oldTest} newTest={newTest} />;

  return <GenericFileDiff change={change} />;
};

export default ChangeVisualization;
