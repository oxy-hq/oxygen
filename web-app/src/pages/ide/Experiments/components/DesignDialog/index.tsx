import { Loader2 } from "lucide-react";
import { get, type Path, useForm } from "react-hook-form";
import ErrorAlert from "@/components/ui/ErrorAlert";
import { Button } from "@/components/ui/shadcn/button";
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogTitle
} from "@/components/ui/shadcn/dialog";
import type { ExperimentDefinition } from "@/types/experiment";
import AboutSection from "./components/AboutSection";
import GuardrailsSection from "./components/GuardrailsSection";
import LeverSection from "./components/LeverSection";
import OutcomeSection from "./components/OutcomeSection";
import PricingSection from "./components/PricingSection";
import ScheduleSection from "./components/ScheduleSection";
import { type DesignFormValues, emptyDesignValues, toFormValues } from "./designForm";
import { designResolver } from "./resolver";
import { placeServerError } from "./serverErrors";
import { type DesignConfig, toConfig } from "./toConfig";

export interface DesignDialogProps {
  title: string;
  description: string;
  submitLabel: string;
  /** The design being edited; absent for a new experiment. */
  initial?: ExperimentDefinition;
  /** Rejections (409 stale or taken, 422 invalid) throw; their message is placed in the form. */
  onSubmit: (config: DesignConfig) => Promise<void>;
}

function DesignForm({
  title,
  description,
  submitLabel,
  initial,
  onSubmit,
  onCancel
}: DesignDialogProps & { onCancel: () => void }) {
  const form = useForm<DesignFormValues, { nameLocked: boolean }>({
    defaultValues: initial ? toFormValues(initial) : emptyDesignValues(),
    resolver: designResolver,
    // An existing experiment keeps the name it has, whatever it is.
    context: { nameLocked: initial !== undefined }
  });
  const { errors, isSubmitting } = form.formState;
  const messageFor = (name: string): string | undefined => {
    const message: unknown = get(errors, name)?.message;
    return typeof message === "string" ? message : undefined;
  };
  const section = { form, messageFor };

  const submit = form.handleSubmit(async (values) => {
    try {
      await onSubmit(toConfig(values).config);
    } catch (e) {
      const placed = placeServerError(
        e instanceof Error ? e.message : "The design could not be saved."
      );
      for (const [field, message] of Object.entries(placed.fields)) {
        form.setError(field as Path<DesignFormValues>, { type: "server", message });
      }
      const general = [...placed.general];
      if (Object.keys(placed.fields).length > 0) {
        general.push(
          general.length > 0
            ? "The other reasons are shown beside their fields."
            : "The reasons are shown beside their fields."
        );
      }
      form.setError("root.server", { type: "server", message: general.join(" ") });
    }
  });

  return (
    <DialogContent
      className='max-h-[85vh] overflow-y-auto sm:max-w-3xl'
      data-testid='experiment-design-dialog'
    >
      <DialogHeader>
        <DialogTitle>{title}</DialogTitle>
        <DialogDescription>{description}</DialogDescription>
      </DialogHeader>
      <form
        onSubmit={submit}
        noValidate
        className='flex flex-col gap-4'
        data-testid='experiment-design-form'
      >
        <AboutSection {...section} nameLocked={initial !== undefined} />
        <OutcomeSection {...section} />
        <LeverSection {...section} />
        <ScheduleSection {...section} />
        <GuardrailsSection {...section} />
        <PricingSection {...section} />
        {errors.root?.server?.message && (
          <div data-testid='experiment-design-error'>
            <ErrorAlert title='The design was not saved' message={errors.root.server.message} />
          </div>
        )}
        <DialogFooter>
          <Button type='button' variant='ghost' onClick={onCancel}>
            Cancel
          </Button>
          <Button type='submit' disabled={isSubmitting} data-testid='experiment-design-submit'>
            {isSubmitting && <Loader2 className='h-4 w-4 animate-spin' />}
            {submitLabel}
          </Button>
        </DialogFooter>
      </form>
    </DialogContent>
  );
}

/**
 * An experiment design as a form, for creating an experiment or editing its
 * draft. The form mounts only while open, so each opening starts from
 * `initial` (or empty). Nothing here shows the design as file text.
 */
export default function DesignDialog({
  open,
  onOpenChange,
  ...props
}: DesignDialogProps & { open: boolean; onOpenChange: (open: boolean) => void }) {
  return (
    <Dialog open={open} onOpenChange={onOpenChange}>
      {open && <DesignForm {...props} onCancel={() => onOpenChange(false)} />}
    </Dialog>
  );
}
