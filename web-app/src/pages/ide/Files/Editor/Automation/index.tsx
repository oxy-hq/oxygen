import { debounce } from "lodash";
import { useMemo, useState } from "react";
import { useSearchParams } from "react-router-dom";
import YAML from "yaml";
import { AutomationForm, type AutomationFormData } from "@/components/automation/AutomationForm";
import { useFileEditorContext } from "@/components/FileEditor/useFileEditorContext";
import { Automation } from "@/pages/automation";
import { useFilesContext } from "../../FilesContext";
import { FilesSubViewMode } from "../../FilesSidebar/constants";
import EditorPageWrapper from "../components/EditorPageWrapper";
import { useEditorContext } from "../contexts/useEditorContext";
import { usePreviewRefresh } from "../usePreviewRefresh";
import ModeSwitcher from "./components/ModeSwitcher";
import { AutomationViewMode } from "./components/types";
import { formDataToYaml, yamlToFormData } from "./formYaml";

const AutomationEditor = () => {
  const { pathb64, gitEnabled } = useEditorContext();
  const { refreshPreview, previewKey } = usePreviewRefresh();
  const { filesSubViewMode } = useFilesContext();

  const [searchParams] = useSearchParams();
  const runId = searchParams.get("run") || undefined;

  const defaultViewMode =
    filesSubViewMode === FilesSubViewMode.OBJECTS
      ? AutomationViewMode.Form
      : AutomationViewMode.Output;

  const [viewMode, setViewMode] = useState<AutomationViewMode>(defaultViewMode);

  return (
    <EditorPageWrapper
      headerPrefixAction={<ModeSwitcher viewMode={viewMode} onViewModeChange={setViewMode} />}
      pathb64={pathb64}
      onSaved={refreshPreview}
      customEditor={viewMode === AutomationViewMode.Form ? <AutomationFormWrapper /> : undefined}
      git={gitEnabled}
      preview={
        <Automation
          key={previewKey + runId}
          pathb64={pathb64}
          runId={runId}
          direction='vertical'
          hideHeader
        />
      }
      previewOnly={viewMode === AutomationViewMode.Output}
    />
  );
};
export default AutomationEditor;

const AutomationFormWrapper = () => {
  const { state, actions } = useFileEditorContext();

  const content = state.content;

  const parsed = useMemo(() => {
    try {
      if (!content) return undefined;
      // A file of only comments parses to null; the form still opens on it.
      return (YAML.parse(content) ?? {}) as Record<string, unknown>;
    } catch (error) {
      console.error("Failed to parse YAML content to form data:", error);
      return undefined;
    }
  }, [content]);

  const data = useMemo(
    () => (parsed ? yamlToFormData(parsed as Partial<AutomationFormData>) : undefined),
    [parsed]
  );

  const onChange = useMemo(
    () =>
      debounce((formData: AutomationFormData) => {
        try {
          const yamlContent = YAML.stringify(formDataToYaml(formData, parsed), {
            indent: 2,
            lineWidth: 0
          });
          actions.setContent(yamlContent);
        } catch (error) {
          console.error("Failed to serialize form data to YAML:", error);
        }
      }, 500),
    [actions, parsed]
  );

  if (!data) return null;

  return <AutomationForm data={data} onChange={onChange} />;
};
