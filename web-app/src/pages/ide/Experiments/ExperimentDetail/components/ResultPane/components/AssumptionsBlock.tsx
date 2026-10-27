import Section from "../../Section";

/**
 * What this design cannot identify, in the server's words, every time. Rendered
 * open and verbatim — never collapsed, never trimmed — and independent of
 * whether the estimate was refused: the assumptions are about the design.
 */
export default function AssumptionsBlock({ assumptions }: { assumptions: string[] }) {
  if (assumptions.length === 0) return null;
  return (
    <Section title='Assumptions this design adds' testId='experiment-report-assumptions'>
      <p className='mb-2 text-muted-foreground text-sm'>
        Neither is identifiable inside the design, so they are stated in every report of it.
      </p>
      <ul className='list-disc pl-4 text-sm'>
        {assumptions.map((a) => (
          <li key={a} className='whitespace-pre-wrap'>
            {a}
          </li>
        ))}
      </ul>
    </Section>
  );
}
