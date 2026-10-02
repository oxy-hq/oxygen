import { cx } from "class-variance-authority";
import type React from "react";
import { isValidElement, type ReactNode } from "react";
import { Prism as SyntaxHighlighter } from "react-syntax-highlighter";
import usePrismTheme from "@/hooks/usePrismTheme";

type CodeBlockProps = {
  children?: ReactNode;
  className?: string;
};

// A fenced block arrives as one string. Raw HTML (rehype-raw) can put elements,
// several nodes or nothing at all inside <code class="language-…">: take the
// text, instead of printing "[object Object]", a comma-joined list or "undefined".
const textOf = (node: ReactNode): string => {
  if (typeof node === "string") return node;
  if (typeof node === "number") return String(node);
  if (Array.isArray(node)) return node.map(textOf).join("");
  if (isValidElement<{ children?: ReactNode }>(node)) return textOf(node.props.children);
  return "";
};

const CodeBlock: React.FC<CodeBlockProps> = ({ children, className }) => {
  const match = /language-(\w+)/.exec(className || "");
  const prismTheme = usePrismTheme();
  return match ? (
    <SyntaxHighlighter
      className={cx(
        "border! m-0! max-h-96! rounded-lg! border-border! bg-editor-background! p-4! font-mono text-sm [&>code]:bg-transparent!",
        className
      )}
      language={match ? match[1] : undefined}
      style={prismTheme}
      PreTag='div'
      lineProps={{ style: { wordBreak: "break-all", whiteSpace: "pre-wrap" } }}
      wrapLines={true}
    >
      {textOf(children)}
    </SyntaxHighlighter>
  ) : (
    <code
      className={cx(
        "border! rounded-lg! border-border! bg-muted! px-1.5 py-0.5 font-mono text-xs",
        className
      )}
    >
      {children}
    </code>
  );
};

export default CodeBlock;
