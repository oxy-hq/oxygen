// Started from pokehouse-oxy `apps/warehouse/src/components/AttachmentsField.tsx` (INVENTORY
// section 2: the only copy, tested by `attachmentsField.test.ts` and `tapTargets.test.ts`; it
// grew past the reference's, oxy-hq/reference-app@3caad1f `src/components/AttachmentsField.tsx`).
// Kept: tiles that carry their own upload state (the scrim and the words while uploading, an
// opaque veil and a Retry target when it failed), Retry's target starting under the remove
// corner so the two 44px targets never overlap, the 44px remove button around a 28px square, the
// phone's visible file name and failure reason (no hover there), the client-side `notice`, the
// picker drawn as a FIELD (it is where files go). The app's constants became props: `max` (its
// MAX_ATTACHMENTS), `accept`, the picker's words and `formatSize`. Changed: the remove square is
// the card in the body ink, not black at 55% with white (raw colours, and white on a translucent
// black over a white photo is the 2.44:1 the app measured for its upload words); a full field
// keeps focus on its picker (`aria-disabled`, not `disabled`, which drops focus to <body> the
// moment the twentieth file lands); removing a file hands focus to the next tile's remove, the
// one before, or the picker, instead of to <body>.
//
// Photos and paperwork attached to a record. The caller uploads as soon as files are picked and
// passes each one back with its status; the field draws them and reports what was pressed.

import { type ReactNode, useLayoutEffect, useRef } from "react";
import { usePhoneLayout } from "../../shell/phone/usePhoneLayout";
import { cx } from "../cx";
import { XIcon } from "../icons";
import { ui } from "../recipes";
import { ArrowClockwiseIcon, FileTextIcon, ImageSquareIcon, WarningCircleIcon } from "./icons";

export type AttachmentStatus = "uploading" | "uploaded" | "failed";

/** One file the field shows. Warehouse's `PendingFile` is one (it has more fields). */
export interface AttachmentFile {
  id: string;
  name: string;
  /** Bytes. */
  size: number;
  /** An object URL or a presigned URL for a picture; none draws a file glyph. */
  previewUrl?: string | null;
  status: AttachmentStatus;
  /** Why an upload failed. */
  error?: string;
}

export interface AttachmentsFieldProps {
  files: readonly AttachmentFile[];
  /** Files were picked. The input is cleared after, so picking the same file again works. */
  onAdd: (list: FileList | null) => void;
  onRemove: (id: string) => void;
  onRetry: (id: string) => void;
  /** Client-side rejections (too big, too many): not upload failures. */
  notice?: ReactNode;
  /** Locks every control (the form is submitting). */
  disabled?: boolean;
  /** The most files the field holds; at the limit the picker says so and takes no more. */
  max?: number;
  /** The file input's `accept`. Default photos and PDFs. */
  accept?: string;
  /** The picker's words, empty and with files. */
  addLabel?: string;
  addMoreLabel?: string;
  /** The picker's words at `max`. Default "Limit reached (20 files)". */
  fullLabel?: string;
  /** A file's size as words. Default `12 KB`, `3.4 MB`. */
  formatSize?: (bytes: number) => string;
  /** Phone layout: the whole name and any failure as visible text. Default `usePhoneLayout()`. */
  phone?: boolean;
  /** The file input's id, for a Field's label, and its message's id. */
  id?: string;
  describedBy?: string;
  className?: string;
}

/** `512 B`, `9.8 KB`, `120 KB`, `3.4 MB`: warehouse's `fmtBytes`. Empty for a nonsense size. */
export function formatFileSize(bytes: number): string {
  if (!Number.isFinite(bytes) || bytes < 0) return "";
  if (bytes < 1024) return `${Math.round(bytes)} B`;
  const kb = bytes / 1024;
  if (kb < 1024) return `${kb < 10 ? kb.toFixed(1) : Math.round(kb)} KB`;
  const mb = kb / 1024;
  return `${mb < 10 ? mb.toFixed(1) : Math.round(mb)} MB`;
}

interface TileProps {
  file: AttachmentFile;
  phone: boolean;
  disabled: boolean;
  formatSize: (bytes: number) => string;
  onRemove: () => void;
  onRetry: () => void;
}

function Tile({ file: f, phone, disabled, formatSize, onRemove, onRetry }: TileProps) {
  const failed = f.status === "failed";
  return (
    <li className='oxy-ui-attachments__tile' data-status={f.status}>
      <div className='oxy-ui-attachments__media'>
        {f.previewUrl ? (
          <img src={f.previewUrl} alt={f.name} className='oxy-ui-attachments__img' />
        ) : (
          <FileTextIcon className='oxy-ui-attachments__file-icon' />
        )}
        {f.status === "uploading" && (
          // The scrim and the words, no spinner: a spinner says "waiting" the same at one
          // second and at thirty. The words ride an opaque card chip in the body ink.
          <div className='oxy-ui-attachments__veil'>
            <span className='oxy-ui-attachments__veil-words'>Uploading…</span>
            <span className='oxy-ui-attachments__sr'> {f.name}</span>
          </div>
        )}
        {failed && (
          <>
            {/* The card, then the error tint over it: the tint alone is translucent. The glyph
                and word sit below the remove corner's 44px. */}
            <div className='oxy-ui-attachments__failed' aria-hidden='true'>
              <div className='oxy-ui-attachments__failed-tint'>
                <ArrowClockwiseIcon className='oxy-ui-attachments__retry-icon' />
                <span>Retry</span>
              </div>
            </div>
            <button
              type='button'
              className={cx("oxy-ui-attachments__retry", ui.focusRingInset)}
              onClick={onRetry}
              disabled={disabled}
              title={f.error}
            >
              <span className='oxy-ui-attachments__sr'>
                Upload of {f.name} failed{f.error ? `: ${f.error}` : ""}. Retry.
              </span>
            </button>
          </>
        )}
      </div>
      {phone ? (
        <p className='oxy-ui-attachments__name oxy-ui-attachments__name--wrap'>{f.name}</p>
      ) : (
        <p className='oxy-ui-attachments__name oxy-ui-attachments__name--cut' title={f.name}>
          {f.name}
        </p>
      )}
      <p className={cx(ui.caption, "oxy-ui-attachments__size")}>{formatSize(f.size)}</p>
      {phone && failed && f.error && <p className='oxy-ui-attachments__reason'>{f.error}</p>}
      {/* 44px to tap, drawn as the 28px square in its corner. */}
      <button
        type='button'
        className={cx("oxy-ui-attachments__remove", ui.focusRingInset)}
        onClick={onRemove}
        disabled={disabled}
        aria-label={`Remove ${f.name}`}
        data-remove=''
      >
        <span className='oxy-ui-attachments__remove-plate'>
          <XIcon />
        </span>
      </button>
    </li>
  );
}

export function AttachmentsField({
  files,
  onAdd,
  onRemove,
  onRetry,
  notice,
  disabled = false,
  max,
  accept = "image/*,application/pdf",
  addLabel = "Add photos or files",
  addMoreLabel = "Add more",
  fullLabel,
  formatSize = formatFileSize,
  phone: phoneProp,
  id,
  describedBy,
  className
}: AttachmentsFieldProps) {
  const detectedPhone = usePhoneLayout();
  const phone = phoneProp ?? detectedPhone;
  const full = max !== undefined && files.length >= max;
  const listRef = useRef<HTMLUListElement>(null);
  const inputRef = useRef<HTMLInputElement>(null);
  // Set when a remove was pressed: the index whose tile should take focus once the list updates.
  const refocus = useRef<number | null>(null);

  // biome-ignore lint/correctness/useExhaustiveDependencies: runs when the list the caller passes changes
  useLayoutEffect(() => {
    const at = refocus.current;
    if (at === null) return;
    refocus.current = null;
    const removes = [...(listRef.current?.querySelectorAll<HTMLElement>("[data-remove]") ?? [])];
    (removes[Math.min(at, removes.length - 1)] ?? inputRef.current)?.focus();
  }, [files]);

  const remove = (index: number, fileId: string) => {
    refocus.current = index;
    onRemove(fileId);
  };

  const pickerWords = full
    ? (fullLabel ?? `Limit reached (${max} files)`)
    : files.length === 0
      ? addLabel
      : addMoreLabel;

  return (
    <div className={cx("oxy-ui-attachments", className)}>
      {files.length > 0 && (
        <ul ref={listRef} className='oxy-ui-attachments__grid'>
          {files.map((f, i) => (
            <Tile
              key={f.id}
              file={f}
              phone={phone}
              disabled={disabled}
              formatSize={formatSize}
              onRemove={() => remove(i, f.id)}
              onRetry={() => onRetry(f.id)}
            />
          ))}
        </ul>
      )}

      {notice ? (
        <p role='alert' className='oxy-ui-attachments__notice'>
          <WarningCircleIcon className={cx(ui.iconInline, "oxy-ui-attachments__notice-icon")} />
          <span>{notice}</span>
        </p>
      ) : null}

      {/* Drawn as a field, not a button: it is where files go. The input it wraps takes the
          focus and is not on screen, so the ring is the label's. */}
      <label
        className={cx(ui.controlLg, "oxy-ui-attachments__picker")}
        data-disabled={full || disabled ? "" : undefined}
      >
        <ImageSquareIcon className='oxy-ui-attachments__picker-icon' />
        {pickerWords}
        <input
          ref={inputRef}
          id={id}
          type='file'
          multiple
          accept={accept}
          className='oxy-ui-attachments__sr'
          aria-describedby={describedBy}
          aria-disabled={full || undefined}
          disabled={disabled}
          onClick={(e) => {
            // Full: the picker keeps focus and refuses, rather than leaving the tab order.
            if (full) e.preventDefault();
          }}
          onChange={(e) => {
            onAdd(e.target.files);
            e.target.value = "";
          }}
        />
      </label>
    </div>
  );
}
