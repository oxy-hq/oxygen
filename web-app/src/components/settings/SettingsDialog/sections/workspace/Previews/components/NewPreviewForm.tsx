import { Plus } from "lucide-react";
import { useForm } from "react-hook-form";
import { Button } from "@/components/ui/shadcn/button";
import { Input } from "@/components/ui/shadcn/input";
import { Label } from "@/components/ui/shadcn/label";
import { useCreatePreview } from "@/hooks/api/workspaces/usePreviews";
import { cannotCompileMessage } from "@/libs/utils/preview";

interface FormValues {
  branch: string;
}

/**
 * The "New preview" control: a branch name, and a button. The server compiles
 * in the background and answers `202`, so the row appears as `compiling` right
 * away and the table polls it to `ready`.
 *
 * A branch the server cannot compile (`409 cannot_compile` — uncommitted edits
 * in its worktree, or no checkout) is refused on the field itself, in the
 * server's words, since that is where the person has to act.
 */
export default function NewPreviewForm({ workspaceId }: { workspaceId: string }) {
  const create = useCreatePreview(workspaceId);
  const {
    register,
    handleSubmit,
    reset,
    setError,
    formState: { errors }
  } = useForm<FormValues>({ defaultValues: { branch: "" } });

  const onSubmit = ({ branch }: FormValues) =>
    create.mutate(branch.trim(), {
      onSuccess: () => reset(),
      onError: (err) => {
        const message = cannotCompileMessage(err);
        if (message) setError("branch", { type: "server", message });
      }
    });

  return (
    <form
      onSubmit={handleSubmit(onSubmit)}
      className='flex flex-col gap-2'
      data-testid='new-preview-form'
      noValidate
    >
      <Label htmlFor='new-preview-branch'>New preview</Label>
      <div className='flex flex-col gap-2 sm:flex-row'>
        <Input
          id='new-preview-branch'
          placeholder='feat/my-change'
          autoComplete='off'
          spellCheck={false}
          className='font-mono sm:flex-1'
          aria-invalid={!!errors.branch}
          data-testid='new-preview-branch'
          {...register("branch", {
            validate: (value) => {
              const branch = value.trim();
              if (!branch) return "Enter a branch name.";
              if (/\s/.test(branch)) return "A branch name can't contain spaces.";
              return true;
            }
          })}
        />
        <Button
          type='submit'
          size='sm'
          disabled={create.isPending}
          data-testid='new-preview-submit'
        >
          <Plus />
          {create.isPending ? "Creating…" : "Create preview"}
        </Button>
      </div>
      {errors.branch && (
        <p role='alert' className='text-destructive text-xs' data-testid='new-preview-error'>
          {errors.branch.message}
        </p>
      )}
    </form>
  );
}
