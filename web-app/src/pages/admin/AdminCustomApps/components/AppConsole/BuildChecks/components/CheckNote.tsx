import type { BuildCheckNote } from "@/types/appBuildChecks";

/**
 * One advisory note — on a check, or the build's summary above the list. Who wrote it
 * (a rule, or which model), its text, and what it points at. Where the org's gate is
 * on, the workspace's own model writes them once a staging attempt settles; a note
 * informs and never changes a check's result.
 */
export const CheckNote = ({
  note,
  "data-testid": testId
}: {
  note: BuildCheckNote;
  "data-testid"?: string;
}) => (
  <div
    className='flex flex-col gap-1 rounded-sm bg-muted/40 px-2 py-1.5 text-xs'
    data-testid={testId}
  >
    <span className='text-[10px] text-muted-foreground uppercase tracking-[0.16em]'>
      {note.author === "model" ? (note.model ?? "Model") : "Rule"} · informs
    </span>
    <p className='whitespace-pre-wrap break-words'>{note.text}</p>
    {note.refs.length > 0 && (
      <div className='flex flex-wrap gap-1'>
        {note.refs.map((ref) =>
          ref.href ? (
            <a
              key={`${ref.kind}:${ref.label}`}
              href={ref.href}
              target='_blank'
              rel='noreferrer'
              className='rounded-sm bg-background px-1 font-mono text-[10px] hover:underline'
            >
              {ref.label}
            </a>
          ) : (
            <span
              key={`${ref.kind}:${ref.label}`}
              className='rounded-sm bg-background px-1 font-mono text-[10px]'
            >
              {ref.label}
            </span>
          )
        )}
      </div>
    )}
  </div>
);
