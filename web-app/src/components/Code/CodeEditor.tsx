import type React from "react";
import { useCallback } from "react";
import EditorModule from "react-simple-code-editor";
import { Prism as SyntaxHighlighter } from "react-syntax-highlighter";
import usePrismTheme from "@/hooks/usePrismTheme";
import { cn } from "@/libs/shadcn/utils";
import { prismLanguage } from "./language";

interface CodeEditorProps {
  value: string;
  onChange: (value: string) => void;
  language?: string;
  /** Cmd/Ctrl+Enter. */
  onExecute?: () => void;
  placeholder?: string;
  className?: string;
}

// The package is CommonJS (`exports.default = Editor`). Vite's dev bundling hands
// the default import back as the module object, Node and the production build as
// the component, so take whichever this is.
const Editor =
  (EditorModule as unknown as { default?: typeof EditorModule }).default ?? EditorModule;

const Passthrough = ({ children }: { children?: React.ReactNode }) => <>{children}</>;

/**
 * A small editable code field: a textarea under a highlighted copy of its own
 * text (react-simple-code-editor), highlighted by the same Prism highlighter
 * as the read-only views.
 */
export default function CodeEditor({
  value,
  onChange,
  language,
  onExecute,
  placeholder,
  className
}: CodeEditorProps) {
  const theme = usePrismTheme();
  const lang = prismLanguage(language);
  const highlight = useCallback(
    (code: string) => (
      <SyntaxHighlighter language={lang} style={theme} PreTag={Passthrough} CodeTag={Passthrough}>
        {code}
      </SyntaxHighlighter>
    ),
    [lang, theme]
  );
  const onKeyDown = (e: React.KeyboardEvent) => {
    if (onExecute && e.key === "Enter" && (e.metaKey || e.ctrlKey)) {
      e.preventDefault();
      onExecute();
    }
  };
  return (
    <div className={cn("h-full overflow-auto bg-editor-background", className)}>
      <Editor
        value={value}
        onValueChange={onChange}
        highlight={highlight}
        onKeyDown={onKeyDown}
        placeholder={placeholder}
        padding={12}
        tabSize={2}
        insertSpaces
        textareaClassName='outline-none'
        className='min-h-full font-mono text-sm'
        data-testid='code-editor'
      />
    </div>
  );
}
