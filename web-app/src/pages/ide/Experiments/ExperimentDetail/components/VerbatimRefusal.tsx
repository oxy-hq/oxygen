import { AlertTriangle } from "lucide-react";
import { Alert, AlertDescription, AlertTitle } from "@/components/ui/shadcn/alert";

/**
 * A refusal is the product of pricing (and of registration, and of the
 * estimator), so it is primary page content, shown exactly as the platform wrote
 * it. It is put in a `<pre>` as a text node: no markdown, no HTML, whitespace
 * kept, and never routed to a toast. The message states only what its own guard
 * tested — reflowing or paraphrasing it would put words in the platform's mouth.
 */
export default function VerbatimRefusal({
  title,
  text,
  testId
}: {
  title: string;
  text: string;
  testId: string;
}) {
  return (
    <Alert variant='destructive' data-testid={testId}>
      <AlertTriangle />
      <AlertTitle>{title}</AlertTitle>
      <AlertDescription>
        <pre
          className='whitespace-pre-wrap break-words font-mono text-foreground text-xs'
          data-testid={`${testId}-text`}
        >
          {text}
        </pre>
      </AlertDescription>
    </Alert>
  );
}
