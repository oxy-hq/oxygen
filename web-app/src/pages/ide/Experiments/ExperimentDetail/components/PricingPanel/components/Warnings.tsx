import { Alert, AlertDescription, AlertTitle } from "@/components/ui/shadcn/alert";

/** The platform's warnings for one candidate (lever, arrival lag, trigger spread, washout 0 …), verbatim. */
export default function Warnings({
  warnings,
  testId = "experiment-price-warnings"
}: {
  warnings: string[];
  testId?: string;
}) {
  if (warnings.length === 0) return null;
  return (
    <Alert data-testid={testId}>
      <AlertTitle>Warnings</AlertTitle>
      <AlertDescription>
        <ul className='list-disc pl-4'>
          {warnings.map((w) => (
            <li key={w} className='whitespace-pre-wrap'>
              {w}
            </li>
          ))}
        </ul>
      </AlertDescription>
    </Alert>
  );
}
