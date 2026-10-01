/**
 * Upload UberEats payment-details reports into a pipeline's landing zone.
 *
 * Rendered as the pipeline page's **Reports** tab, only for a source kind
 * that accepts uploads — the endpoint refuses any other, so showing it
 * elsewhere would offer an action that cannot succeed.
 *
 * # Landing a file is not running the pipeline
 *
 * Upload writes the object and stops. That is the model every ELT tool
 * converged on — Fivetran sunset its browser upload in favour of watching a
 * folder, Airbyte never had one — and it keeps two failures apart: a report
 * the server refused is the uploader's problem, a run that failed is not.
 * "Run after uploading" is offered as an explicit opt-in because the common
 * case really is "I dropped the month in, go", and making someone switch tabs
 * for it is friction with no safety value.
 *
 * # Why the id is the content hash
 *
 * The object key is the loader's merge identity, so `workflow_id` is a hash of
 * the bytes: re-dropping the same file lands on the same key and merges, while
 * a different file gets its own. A random id would turn every re-drop into a
 * duplicate — the same replace-by-file-sha reasoning the bookkeeping app's
 * importer uses.
 */

import { AlertTriangle, CheckCircle2, FileUp, Loader2, UploadCloud, X } from "lucide-react";
import { useCallback, useMemo, useRef, useState } from "react";
import { toast } from "sonner";
import { Button } from "@/components/ui/shadcn/button";
import { Checkbox } from "@/components/ui/shadcn/checkbox";
import { Input } from "@/components/ui/shadcn/input";
import { Label } from "@/components/ui/shadcn/label";
import { useStartAirwayRun } from "@/hooks/api/airway/useAirway";
import { useUploadReport } from "@/hooks/api/airway/useUploadReport";
import useCurrentProjectBranch from "@/hooks/useCurrentProjectBranch";
import { cn } from "@/libs/shadcn/utils";
import type { UploadedReport } from "@/services/api/airway";

/** One file's journey, so a batch can report per-file rather than all-or-nothing. */
type Item = {
  /**
   * Stable across removals. The index cannot serve: dropping one file shifts
   * every later index, so React would re-key the survivors and the remove
   * button would target the wrong row.
   */
  id: string;
  file: File;
  status: "pending" | "uploading" | "done" | "failed";
  /** Airway's own message when it refuses — it names the file and the column. */
  error?: string;
  result?: UploadedReport;
};

/**
 * Mirrors `MAX_REPORT_BYTES` in `source_upload.rs`.
 *
 * Checked here so an oversized file is named before it is sent. Past this the
 * request dies in axum's `DefaultBodyLimit` layer, which answers with its own
 * terse body rather than the handler's 413 — so the user uploads the whole
 * file and then gets a message that does not mention size.
 *
 * The server stays the authority; this only saves the round trip.
 */
const MAX_REPORT_BYTES = 64 * 1024 * 1024;

/**
 * A list key, unique within this component's lifetime.
 *
 * Deliberately NOT `crypto.randomUUID`. That is secure-context-only, exactly
 * like `crypto.subtle`, so on a plain-HTTP self-hosted `oxy serve` it is
 * `undefined` and the drop handler threw before an item existed — which put
 * the failure *upstream* of the guard written to explain it, leaving the user
 * with the opaque `TypeError` the guard was meant to replace.
 *
 * This is a React key and a removal handle, never persisted and never sent, so
 * a counter is sufficient; the identity that matters to the server is the
 * content hash. Hashing still needs the secure context, and `contentHash` says
 * so plainly at upload time.
 */
let itemSeq = 0;
function nextItemId(): string {
  itemSeq += 1;
  return `item-${itemSeq}`;
}

/** `2026.08 UberEats SF.csv` → `2026-08`, for the period field's default. */
function periodFromName(name: string): string | undefined {
  const m = /^(\d{4})[.\-_](\d{2})(?!\d)/.exec(name);
  if (!m) return undefined;
  const month = Number(m[2]);
  if (month < 1 || month > 12) return undefined;
  return `${m[1]}-${m[2]}`;
}

const ReportUpload: React.FC<{ pipelineRef: string }> = ({ pipelineRef }) => {
  // Own its project context rather than take it as a prop, matching
  // `QuickBooksReconnect` — the pipeline page does not thread one today, and a
  // prop would make every future embedder pass something it can look up.
  const { project } = useCurrentProjectBranch();
  const projectId = project.id;
  const [items, setItems] = useState<Item[]>([]);
  const [period, setPeriod] = useState("");
  const [runAfter, setRunAfter] = useState(false);
  const [busy, setBusy] = useState(false);
  const [dragging, setDragging] = useState(false);
  /**
   * Has the user typed in the period field?
   *
   * Without it, an inferred value is indistinguishable from a deliberate one,
   * so the choice is between overwriting what someone typed and letting a
   * guess outlive the drop that produced it. The second is how a July report
   * gets stamped August.
   */
  const [periodTouched, setPeriodTouched] = useState(false);
  const inputRef = useRef<HTMLInputElement>(null);
  const startRun = useStartAirwayRun();
  const uploadOne = useUploadReport();

  const add = useCallback(
    (files: FileList | null) => {
      if (!files?.length) return;
      const picked = Array.from(files);

      const oversized = picked.filter((f) => f.size > MAX_REPORT_BYTES);
      if (oversized.length > 0) {
        const mb = Math.round(MAX_REPORT_BYTES / (1024 * 1024));
        toast.error(
          `${oversized.map((f) => f.name).join(", ")} — over the ${mb} MB limit for one report`
        );
      }
      const accepted = picked.filter((f) => f.size <= MAX_REPORT_BYTES);
      if (accepted.length === 0) return;

      // The new items are built OUTSIDE the `setItems` updater, which React
      // requires to be pure — it is re-invoked under StrictMode, and both
      // `setPeriod` and `nextItemId`'s counter were running in there. Nothing
      // user-visible came of it (a duplicate `setPeriod` with the same value,
      // an id counter advancing twice), but it is the wiring that acquires a
      // bug the moment someone adds a non-idempotent line beside it.
      const fresh = accepted.map((file): Item => ({ id: nextItemId(), file, status: "pending" }));

      // Written FUNCTIONALLY even though `fresh` is already computed, so this
      // does not depend on when React flushes. A snapshot write (`setItems`
      // with an array built from the `items` closure) is correct only while a
      // render is guaranteed to commit between the upload loop's writes and
      // the next drop — true today, but an invariant this file would be
      // relying on rather than enforcing. `upload()` writes functionally for
      // the same reason, and the two should not disagree about it. The stakes
      // are small (a `done` item reverting to `pending` and being re-sent,
      // which the content-hash key makes idempotent) but the cost is nil.
      setItems((prev) => [...prev, ...fresh]);

      // The vote below reads the closure's `items` instead. That is sound
      // where the write is not: a period default is advisory, so a stale read
      // costs a default the user can see and change, not a lost item.
      //
      // The recompute-on-next-drop is the weaker half of that argument. What
      // closes most of the window is `disagreeing`, which recomputes over the
      // COMMITTED `items` on every render and carries no `periodTouched`
      // guard — so a missed file whose name gives a DIFFERENT month surfaces
      // as "N file(s) name a different month" rather than going quiet until
      // someone drops again.
      //
      // Exactly that far and no further: `disagreeing` matches on a name that
      // parses, so a missed file naming NO month is invisible to it. That is
      // failure #2 below, and it rests on the vote alone — which handles it by
      // blanking rather than by warning. Both gaps need two drops inside one
      // unflushed window, which React's per-event batching does not produce,
      // so this is about the comment being exact rather than a live hole.
      const next = [...items, ...fresh];

      // Default the period only when every still-pending name AGREES, so a
      // conventionally-named batch needs no typing — and only while the user
      // has not typed one, because a value they entered outranks any guess.
      //
      // Three failures live here, each one step out from the last:
      //
      // 1. Taking the FIRST parseable name applied one value to every file, so
      //    `2026.07 …` and `2026.08 …` dropped together stamped August July.
      // 2. Filtering unparseable names out before the vote let one named file
      //    plus one renamed `payment-details.csv` "agree" — and the unnamed
      //    one would otherwise have been REFUSED by the server for having no
      //    derivable period, so the guess converted an honest refusal into a
      //    silent wrong month. A name that says nothing does not agree.
      // 3. Voting over one `add()` call let the value outlive its drop:
      //    `prev || agreed` cannot tell a typed value from one inferred a drop
      //    ago, so a later `2026.07 …` lost to an earlier `2026.08`. Hence the
      //    vote runs over `next`, and `periodTouched` carries the distinction
      //    `prev ||` could not.
      //
      // None of the three is refused downstream: both halves are present and
      // in range, and the key is a content hash, so the report merges cleanly
      // into the wrong period. Blank is the safe default — the server then
      // derives the period from each file's own name.
      //
      // A value the user TYPED is deliberately NOT overwritten here — that is
      // what `periodTouched` protects. Its staleness is surfaced instead, by
      // `disagreeing` below, because silently preferring the field is the same
      // wrong month by another route and silently preferring the name would
      // discard what someone entered on purpose.
      //
      // `done` items are excluded: their period is spent, and a report that
      // has already landed should not hold the field for the next one.
      if (!periodTouched) {
        const named = next
          .filter((it) => it.status !== "done")
          .map((it) => periodFromName(it.file.name));
        setPeriod(named.length > 0 && named.every((x) => x === named[0]) ? (named[0] ?? "") : "");
      }
    },
    [items, periodTouched]
  );

  const parsedPeriod = useMemo(() => {
    const m = /^(\d{4})-(\d{2})$/.exec(period.trim());
    if (!m) return undefined;
    return { year: Number(m[1]), month: Number(m[2]) };
  }, [period]);

  const periodInvalid = period.trim() !== "" && !parsedPeriod;
  const pending = items.filter((i) => i.status === "pending" || i.status === "failed");

  /**
   * Pending files whose own name names a different month than the field.
   *
   * The last member of the wrong-month family, and the mildest: `periodTouched`
   * is set once and never cleared, so a period the user typed for one batch
   * still applies to the next drop. Type `2026-07`, upload, then drop
   * `2026.08 …csv` and August is stamped July — not refused, because both
   * halves are present and in range and the content-hash key is distinct.
   *
   * Surfaced rather than resolved, because both automatic resolutions are
   * wrong: preferring the field is the silent wrong month again, and
   * preferring the name discards a value someone entered on purpose. The user
   * is the only one who knows which is right, so they are told and left to
   * decide. It catches a plain typo on the same evidence.
   */
  const disagreeing = useMemo(() => {
    const typed = period.trim();
    if (!typed || !parsedPeriod) return [];
    return items
      .filter((it) => it.status !== "done")
      .filter((it) => {
        const named = periodFromName(it.file.name);
        return named !== undefined && named !== typed;
      });
  }, [items, period, parsedPeriod]);

  const upload = useCallback(async () => {
    setBusy(true);
    let uploaded = 0;

    // Sequential, not parallel. A batch is a handful of small files, and one
    // request at a time keeps the per-file status honest and the server's
    // validation errors attributable.
    for (const item of items) {
      if (item.status === "done") continue;
      setItems((prev) =>
        prev.map((it) => (it.id === item.id ? { ...it, status: "uploading" } : it))
      );
      try {
        const result = await uploadOne.mutateAsync({
          projectId,
          pipelineRef,
          file: item.file,
          period: parsedPeriod
        });
        uploaded += 1;
        setItems((prev) =>
          prev.map((it) =>
            it.id === item.id ? { ...it, status: "done", result, error: undefined } : it
          )
        );
      } catch (e) {
        // The server's message is shown verbatim: it names the file and the
        // missing JE-critical column, and rewording it here would make the
        // upload-time and load-time diagnoses differ for one cause.
        // `||`, not `??`: the role guards reject with a bare `StatusCode` and
        // therefore an EMPTY body, and `??` only falls through on
        // null/undefined — so `data === ""` won and a Viewer hitting
        // `WorkspaceEditor` got a red triangle with no text at all.
        const error =
          (e as { response?: { data?: string } })?.response?.data ||
          (e as Error)?.message ||
          "upload failed";
        setItems((prev) =>
          prev.map((it) =>
            it.id === item.id ? { ...it, status: "failed", error: String(error) } : it
          )
        );
      }
    }

    setBusy(false);

    // Only when something landed. Running after a batch that entirely failed
    // would read as "it worked" — the run would succeed over unchanged data.
    if (runAfter && uploaded > 0) {
      try {
        await startRun.mutateAsync({ pipeline_ref: pipelineRef });
        toast.success(`Uploaded ${uploaded} report(s) — pipeline run started`);
      } catch {
        // Deliberately not a failure of the upload: the reports ARE in the
        // zone, and saying otherwise would invite a re-upload that is not
        // needed.
        toast.warning(`Uploaded ${uploaded} report(s), but the run could not be started`);
      }
    } else if (uploaded > 0) {
      toast.success(`Uploaded ${uploaded} report(s)`);
    }
  }, [items, parsedPeriod, pipelineRef, projectId, runAfter, startRun, uploadOne]);

  return (
    <div className='flex flex-col gap-4 p-4'>
      {/* biome-ignore lint/a11y/noStaticElementInteractions: the drop target is
          a region, and the button below is the keyboard-reachable equivalent. */}
      <div
        className={cn(
          "flex flex-col items-center justify-center gap-2 rounded-lg border border-dashed p-8 text-center",
          dragging ? "border-primary bg-accent/40" : "border-border"
        )}
        onDragOver={(e) => {
          e.preventDefault();
          setDragging(true);
        }}
        onDragLeave={() => setDragging(false)}
        onDrop={(e) => {
          e.preventDefault();
          setDragging(false);
          add(e.dataTransfer.files);
        }}
      >
        <UploadCloud className='h-8 w-8 text-muted-foreground' />
        <p className='text-sm'>Drop payment-details reports here</p>
        <p className='text-muted-foreground text-xs'>
          CSV only. Each report is validated before it lands — a renamed column is refused here
          rather than failing a run later.
        </p>
        <Button variant='outline' size='sm' onClick={() => inputRef.current?.click()}>
          <FileUp className='mr-2 h-4 w-4' />
          Choose files
        </Button>
        <input
          ref={inputRef}
          type='file'
          accept='.csv,text/csv'
          multiple
          className='hidden'
          onChange={(e) => {
            add(e.target.files);
            // Reset so re-picking the same file fires `change` again.
            e.target.value = "";
          }}
        />
      </div>

      <div className='flex flex-wrap items-end gap-4'>
        <div className='flex flex-col gap-1'>
          <Label htmlFor='report-period' className='text-xs'>
            Period
          </Label>
          <Input
            id='report-period'
            value={period}
            placeholder='YYYY-MM'
            className={cn("w-36", periodInvalid && "border-destructive")}
            onChange={(e) => {
              setPeriodTouched(true);
              setPeriod(e.target.value);
            }}
          />
          <span
            className={cn(
              "text-xs",
              disagreeing.length > 0 ? "text-destructive" : "text-muted-foreground"
            )}
          >
            {periodInvalid
              ? "Use YYYY-MM"
              : disagreeing.length > 0
                ? `${disagreeing.length} file(s) name a different month (${[
                    ...new Set(
                      disagreeing.map((it) => periodFromName(it.file.name)).filter(Boolean)
                    )
                  ].join(", ")}) — this field wins`
                : parsedPeriod
                  ? "Applied to every file in this batch"
                  : "Left blank — read from each file name"}
          </span>
        </div>

        <div className='flex items-center gap-2 pb-6'>
          <Checkbox
            id='run-after'
            checked={runAfter}
            onCheckedChange={(v) => setRunAfter(v === true)}
          />
          <Label htmlFor='run-after' className='font-normal text-sm'>
            Run the pipeline after uploading
          </Label>
        </div>

        <Button
          className='mb-6'
          disabled={busy || periodInvalid || pending.length === 0}
          onClick={upload}
        >
          {busy && <Loader2 className='mr-2 h-4 w-4 animate-spin' />}
          Upload {pending.length > 0 ? `${pending.length} file(s)` : ""}
        </Button>
      </div>

      {items.length > 0 && (
        <ul className='flex flex-col gap-2'>
          {items.map((item) => (
            <li key={item.id} className='flex items-start gap-3 rounded-md border p-3 text-sm'>
              <span className='mt-0.5'>
                {item.status === "uploading" && <Loader2 className='h-4 w-4 animate-spin' />}
                {item.status === "done" && <CheckCircle2 className='h-4 w-4 text-success' />}
                {item.status === "failed" && <AlertTriangle className='h-4 w-4 text-destructive' />}
                {item.status === "pending" && <FileUp className='h-4 w-4 text-muted-foreground' />}
              </span>
              <div className='min-w-0 flex-1'>
                <p className='truncate font-medium'>{item.file.name}</p>
                {item.result && (
                  <p className='text-muted-foreground text-xs'>
                    {item.result.report_year}-{String(item.result.report_month).padStart(2, "0")} ·{" "}
                    {item.result.rows} row(s)
                    {/* Zero rows is a successful upload of an empty report,
                        which is worth seeing rather than hiding. */}
                  </p>
                )}
                {item.error && (
                  <p className='whitespace-pre-wrap text-destructive text-xs'>{item.error}</p>
                )}
              </div>
              {item.status !== "uploading" && (
                <Button
                  variant='ghost'
                  size='icon'
                  className='h-6 w-6'
                  onClick={() => setItems((prev) => prev.filter((it) => it.id !== item.id))}
                >
                  <X className='h-3 w-3' />
                </Button>
              )}
            </li>
          ))}
        </ul>
      )}
    </div>
  );
};

export default ReportUpload;
