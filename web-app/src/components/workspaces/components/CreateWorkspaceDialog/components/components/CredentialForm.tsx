import { ArrowRight, CheckCircle2, FileIcon, Loader2, Trash2, UploadCloud } from "lucide-react";
import { type ChangeEvent, type DragEvent, useCallback, useRef, useState } from "react";
import { SecretInput } from "@/components/ui/SecretInput";
import { Button } from "@/components/ui/shadcn/button";
import { Input } from "@/components/ui/shadcn/input";
import { Textarea } from "@/components/ui/shadcn/textarea";
import { cn } from "@/libs/shadcn/utils";
import type { CredentialField } from "../types";

/**
 * Per-field state for `file_upload` fields. Files the user has picked stay in
 * `selected` until the user clicks the form CTA — only then does the actual
 * upload happen. After a successful upload, the server-confirmed paths land
 * in `uploadedPaths`. One instance is keyed per field, so a form can
 * hypothetically carry multiple upload fields — today only DuckDB uses this.
 */
interface FileUploadState {
  selected: File[];
  uploading: boolean;
  uploadedPaths: string[];
  error?: string;
}

/** Result returned by the parent's upload callback. */
interface FileUploadResult {
  subdir: string;
  files: string[];
}

interface CredentialFormProps {
  fields: CredentialField[];
  buttonLabel?: string;
  onSubmit: (values: Record<string, string>) => void;
  disabled?: boolean;
  initialValues?: Record<string, string>;
  /**
   * Pre-populated uploaded paths per `file_upload` field key. Lets the form
   * remember files an earlier visit already sent so the user isn't blocked
   * with a disabled CTA after navigating back.
   */
  initialUploadedFiles?: Record<string, string[]>;
  /**
   * Inline error surfaced below the form. Used by the GitHub onboarding flow
   * to show a failed connection-test message so the user can fix typos and
   * retry without losing what they've already typed.
   */
  errorMessage?: string;
  /**
   * Invoked when the user clicks the form CTA with pending file selections.
   * Should upload the files and resolve with the server-assigned subdir + file
   * paths, or reject (the form surfaces the error inline). Files are NOT
   * uploaded on drag/drop or pick — the form holds them locally so the user
   * can remove mistakes before committing.
   */
  onFileUpload?: (files: File[]) => Promise<FileUploadResult | null>;
}

export default function CredentialForm({
  fields,
  buttonLabel = "Continue",
  onSubmit,
  disabled,
  initialValues,
  initialUploadedFiles,
  errorMessage,
  onFileUpload
}: CredentialFormProps) {
  const [values, setValues] = useState<Record<string, string>>(() => {
    const initial: Record<string, string> = {};
    for (const field of fields) {
      // Prefer initialValues (from previous attempt), then defaultValue
      if (initialValues?.[field.key]) {
        initial[field.key] = initialValues[field.key];
      } else if (field.defaultValue) {
        initial[field.key] = field.defaultValue;
      }
    }
    return initial;
  });

  const [uploads, setUploads] = useState<Record<string, FileUploadState>>(() => {
    if (!initialUploadedFiles) return {};
    const seeded: Record<string, FileUploadState> = {};
    for (const [key, paths] of Object.entries(initialUploadedFiles)) {
      if (paths.length === 0) continue;
      seeded[key] = { selected: [], uploading: false, uploadedPaths: paths };
    }
    return seeded;
  });

  const handleChange = useCallback((key: string, value: string) => {
    setValues((prev) => ({ ...prev, [key]: value }));
  }, []);

  const isFieldFilled = useCallback(
    (field: CredentialField) => {
      if (!field.required) return true;
      if (field.type === "file_upload") {
        const state = uploads[field.key];
        if (!state || state.uploading) return false;
        // Pending picks only count if a handler can actually upload them —
        // otherwise the CTA would submit with the field value still empty.
        if (state.selected.length > 0 && onFileUpload) return true;
        return state.uploadedPaths.length > 0;
      }
      const v = values[field.key];
      return typeof v === "string" && v.trim() !== "";
    },
    [uploads, values, onFileUpload]
  );

  /** Per-field validation message, or undefined when valid / empty. */
  const validationError = useCallback(
    (field: CredentialField): string | undefined => {
      if (!field.validateAs) return undefined;
      const raw = values[field.key];
      if (typeof raw !== "string" || raw.trim() === "") return undefined;
      if (field.validateAs === "json") {
        try {
          JSON.parse(raw);
        } catch {
          return "Must be valid JSON.";
        }
      }
      return undefined;
    },
    [values]
  );

  const allRequiredFilled = fields.every(isFieldFilled);
  const allValid = fields.every((f) => validationError(f) === undefined);
  const anyUploading = Object.values(uploads).some((s) => s.uploading);

  const addSelectedFiles = useCallback((fieldKey: string, files: File[]) => {
    if (files.length === 0) return;
    setUploads((prev) => {
      const current = prev[fieldKey] ?? {
        selected: [],
        uploading: false,
        uploadedPaths: []
      };
      // Drop dupes (same name + size) so re-picking the same file doesn't
      // create a phantom row the user can't tell apart from the original.
      const seen = new Set(current.selected.map((f) => `${f.name}:${f.size}`));
      const deduped = files.filter((f) => !seen.has(`${f.name}:${f.size}`));
      return {
        ...prev,
        [fieldKey]: {
          ...current,
          selected: [...current.selected, ...deduped],
          error: undefined
        }
      };
    });
  }, []);

  const removeSelectedFile = useCallback((fieldKey: string, index: number) => {
    setUploads((prev) => {
      const current = prev[fieldKey];
      if (!current) return prev;
      const next = [...current.selected];
      next.splice(index, 1);
      return { ...prev, [fieldKey]: { ...current, selected: next } };
    });
  }, []);

  const handleSubmit = useCallback(async () => {
    if (!allRequiredFilled || !allValid || anyUploading) return;

    // Upload any pending file_upload selections first, then submit with the
    // server-chosen subdir as the field value (DuckDB uses this as
    // `file_search_path`). Bail out without submitting if an upload fails.
    let updatedValues = values;
    for (const field of fields) {
      if (field.type !== "file_upload") continue;
      const state = uploads[field.key];
      if (!state?.selected.length || !onFileUpload) continue;

      const filesToUpload = state.selected;
      setUploads((prev) => ({
        ...prev,
        [field.key]: { ...prev[field.key]!, uploading: true, error: undefined }
      }));

      try {
        const result = await onFileUpload(filesToUpload);
        if (!result) {
          setUploads((prev) => ({
            ...prev,
            [field.key]: { ...prev[field.key]!, uploading: false, error: "Upload failed." }
          }));
          return;
        }
        setUploads((prev) => {
          const current = prev[field.key];
          if (!current) return prev;
          return {
            ...prev,
            [field.key]: {
              selected: [],
              uploading: false,
              uploadedPaths: [...current.uploadedPaths, ...result.files]
            }
          };
        });
        updatedValues = { ...updatedValues, [field.key]: result.subdir };
        handleChange(field.key, result.subdir);
      } catch (err) {
        setUploads((prev) => ({
          ...prev,
          [field.key]: {
            ...prev[field.key]!,
            uploading: false,
            error: err instanceof Error ? err.message : "Upload failed."
          }
        }));
        return;
      }
    }

    onSubmit(updatedValues);
  }, [
    allRequiredFilled,
    allValid,
    anyUploading,
    fields,
    uploads,
    values,
    onFileUpload,
    onSubmit,
    handleChange
  ]);

  return (
    <div className='flex flex-col gap-3'>
      <div className='grid grid-cols-2 gap-3'>
        {fields.map((field) => {
          const inputId = `credential-${field.key}`;
          if (field.type === "file_upload") {
            return (
              <div key={field.key} className='col-span-2'>
                <label htmlFor={inputId} className='mb-1 block text-muted-foreground text-xs'>
                  {field.label}
                  {field.required && <span className='text-destructive'> *</span>}
                </label>
                <FileUploadField
                  inputId={inputId}
                  placeholder={field.placeholder}
                  helperText={field.helperText}
                  accept={field.accept ?? ""}
                  multiple={field.multiple ?? false}
                  disabled={disabled}
                  state={uploads[field.key]}
                  onPick={(files) => addSelectedFiles(field.key, files)}
                  onRemoveSelected={(index) => removeSelectedFile(field.key, index)}
                />
              </div>
            );
          }
          if (field.type === "textarea") {
            const fieldError = validationError(field);
            return (
              <div key={field.key} className='col-span-2'>
                <label htmlFor={inputId} className='mb-1 block text-muted-foreground text-xs'>
                  {field.label}
                  {field.required && <span className='text-destructive'> *</span>}
                </label>
                <Textarea
                  id={inputId}
                  value={values[field.key] ?? ""}
                  onChange={(e) => handleChange(field.key, e.target.value)}
                  placeholder={field.placeholder}
                  disabled={disabled}
                  rows={field.rows ?? 4}
                  aria-invalid={fieldError ? true : undefined}
                  className='font-mono text-sm'
                />
                {fieldError && <p className='mt-1 text-destructive text-xs'>{fieldError}</p>}
              </div>
            );
          }
          const isPassword = field.type === "password";
          const InputComponent = isPassword ? SecretInput : Input;
          return (
            <div key={field.key} className={isPassword ? "col-span-2" : undefined}>
              <label htmlFor={inputId} className='mb-1 block text-muted-foreground text-xs'>
                {field.label}
                {field.required && <span className='text-destructive'> *</span>}
              </label>
              <InputComponent
                id={inputId}
                value={values[field.key] ?? ""}
                onChange={(e) => handleChange(field.key, e.target.value)}
                placeholder={field.placeholder}
                disabled={disabled}
                className='font-mono text-sm'
                onKeyDown={(e) => {
                  // handleSubmit cannot reject: a failed upload is caught and shown on its field.
                  if (e.key === "Enter" && allRequiredFilled && !disabled) void handleSubmit();
                }}
              />
            </div>
          );
        })}
      </div>
      {errorMessage && (
        <p className='rounded-md border border-destructive/40 bg-destructive/10 px-3 py-2 text-destructive text-xs'>
          {errorMessage}
        </p>
      )}
      <div className='flex justify-end'>
        <Button
          onClick={handleSubmit}
          disabled={disabled || !allRequiredFilled || !allValid || anyUploading}
          size='sm'
        >
          {anyUploading ? <Loader2 className='size-3 animate-spin' /> : null}
          {anyUploading ? "Uploading…" : buttonLabel}
          {!anyUploading && <ArrowRight className='size-3' />}
        </Button>
      </div>
    </div>
  );
}

interface FileUploadFieldProps {
  inputId: string;
  placeholder: string;
  helperText?: string;
  accept: string;
  multiple: boolean;
  disabled?: boolean;
  state?: FileUploadState;
  onPick: (files: File[]) => void;
  onRemoveSelected: (index: number) => void;
}

function FileUploadField({
  inputId,
  placeholder,
  helperText,
  accept,
  multiple,
  disabled,
  state,
  onPick,
  onRemoveSelected
}: FileUploadFieldProps) {
  const inputRef = useRef<HTMLInputElement | null>(null);
  const [isDragging, setIsDragging] = useState(false);

  const handleFiles = useCallback(
    (list: FileList | null) => {
      if (!list || list.length === 0) return;
      onPick(Array.from(list));
    },
    [onPick]
  );

  const handleDrop = useCallback(
    (e: DragEvent<HTMLButtonElement>) => {
      e.preventDefault();
      e.stopPropagation();
      setIsDragging(false);
      if (disabled) return;
      handleFiles(e.dataTransfer?.files ?? null);
    },
    [disabled, handleFiles]
  );

  const handleDragOver = useCallback(
    (e: DragEvent<HTMLButtonElement>) => {
      e.preventDefault();
      e.stopPropagation();
      if (disabled) return;
      setIsDragging(true);
    },
    [disabled]
  );

  const handleDragLeave = useCallback((e: DragEvent<HTMLButtonElement>) => {
    e.preventDefault();
    e.stopPropagation();
    setIsDragging(false);
  }, []);

  const handleChange = useCallback(
    (e: ChangeEvent<HTMLInputElement>) => {
      handleFiles(e.target.files);
      // Reset so picking the same file again still fires change.
      e.target.value = "";
    },
    [handleFiles]
  );

  const selectedFiles = state?.selected ?? [];
  const uploadedPaths = state?.uploadedPaths ?? [];
  const hasSelected = selectedFiles.length > 0;
  const hasUploaded = uploadedPaths.length > 0;
  const isUploading = state?.uploading ?? false;

  return (
    <div className='flex flex-col gap-2'>
      <button
        type='button'
        onClick={() => inputRef.current?.click()}
        onDragOver={handleDragOver}
        onDragLeave={handleDragLeave}
        onDrop={handleDrop}
        disabled={disabled || isUploading}
        className={cn(
          "flex min-h-24 w-full flex-col items-center justify-center gap-2 rounded-md border border-border border-dashed bg-muted/30 px-4 py-6 text-muted-foreground text-sm transition-colors hover:border-primary hover:text-foreground",
          isDragging && "border-primary bg-muted text-foreground",
          disabled && "cursor-not-allowed opacity-60"
        )}
      >
        {isUploading ? (
          <Loader2 className='h-5 w-5 animate-spin' />
        ) : (
          <UploadCloud className='h-5 w-5' />
        )}
        <span className='text-center'>{isUploading ? "Uploading…" : placeholder}</span>
        {accept && (
          <span className='text-muted-foreground text-xs'>
            Accepts{" "}
            {accept
              .split(",")
              .map((a) => a.trim())
              .join(" and ")}
          </span>
        )}
      </button>
      <input
        ref={inputRef}
        id={inputId}
        type='file'
        accept={accept}
        multiple={multiple}
        className='hidden'
        onChange={handleChange}
        disabled={disabled || isUploading}
      />
      {helperText && <p className='text-muted-foreground text-xs'>{helperText}</p>}
      {state?.error && <p className='text-destructive text-xs'>{state.error}</p>}
      {(hasSelected || hasUploaded) && (
        <ul className='flex flex-col gap-1'>
          {selectedFiles.map((file, index) => (
            <li
              // addSelectedFiles dedupes by name+size, so the pair is unique
              // within `selected` and stable across sibling removals.
              key={`${file.name}-${file.size}`}
              className='flex items-center gap-2 rounded border border-border bg-muted/40 px-2 py-1 text-xs'
            >
              <FileIcon className='h-3 w-3 text-muted-foreground' />
              <span className='flex-1 truncate font-mono'>{file.name}</span>
              <Button
                type='button'
                variant='ghost'
                size='icon'
                className='size-5 text-muted-foreground hover:text-destructive'
                onClick={() => onRemoveSelected(index)}
                disabled={disabled || isUploading}
                aria-label={`Remove ${file.name}`}
              >
                <Trash2 className='size-3' />
              </Button>
            </li>
          ))}
          {uploadedPaths.map((path) => (
            <li
              key={path}
              className='flex items-center gap-2 rounded border border-border bg-muted/40 px-2 py-1 text-xs'
            >
              <CheckCircle2 className='size-3 text-success' />
              <span className='flex-1 truncate font-mono'>{path}</span>
            </li>
          ))}
        </ul>
      )}
    </div>
  );
}
