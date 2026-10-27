/** What an aborted run did with its levers, in one plain line. */
export default function AbortLeversNote({ kept }: { kept: boolean }) {
  return (
    <p data-testid='experiment-abort-levers'>
      {kept
        ? "Levers were left as they were when the run was aborted; the experiment no longer manages them."
        : "Levers were turned off when the run was aborted."}
    </p>
  );
}
