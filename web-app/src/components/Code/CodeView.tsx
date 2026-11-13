import { Prism as SyntaxHighlighter } from "react-syntax-highlighter";
import usePrismTheme from "@/hooks/usePrismTheme";
import { cn } from "@/libs/shadcn/utils";
import { prismLanguage } from "./language";

interface CodeViewProps {
  value: string;
  /** A language name (`sql`, `yaml`, `json`, …); unknown ones render as text. */
  language?: string;
  /** The first line's number, for a snippet of a larger file. */
  startLine?: number;
  showLineNumbers?: boolean;
  className?: string;
}

/**
 * Read-only, syntax-highlighted code. Fills its container and scrolls. The
 * same Prism highlighter as Markdown code blocks, so there is one highlighter
 * and one theme across the app.
 */
export default function CodeView({
  value,
  language,
  startLine = 1,
  showLineNumbers = true,
  className
}: CodeViewProps) {
  const theme = usePrismTheme();
  return (
    <div className={cn("h-full overflow-auto bg-editor-background", className)}>
      <SyntaxHighlighter
        language={prismLanguage(language)}
        style={theme}
        showLineNumbers={showLineNumbers}
        startingLineNumber={startLine}
        wrapLongLines
        customStyle={{
          margin: 0,
          padding: "0.75rem 0",
          background: "transparent",
          fontSize: "0.875rem"
        }}
        lineNumberStyle={{ minWidth: "3em", opacity: 0.5 }}
      >
        {value}
      </SyntaxHighlighter>
    </div>
  );
}
