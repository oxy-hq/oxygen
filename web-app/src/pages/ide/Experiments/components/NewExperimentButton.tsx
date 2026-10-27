import { Plus } from "lucide-react";
import { useState } from "react";
import { useNavigate } from "react-router-dom";
import { toast } from "sonner";
import { Button } from "@/components/ui/shadcn/button";
import { useCreateExperiment } from "@/hooks/api/useExperiments";
import { useRole } from "@/hooks/useRole";
import type { ExperimentDesignInput } from "@/types/experiment";
import { useExperimentRoutes } from "../useExperimentRoutes";
import DesignDialog from "./DesignDialog";

/** Creates an experiment (as a draft design) from the design form. */
export default function NewExperimentButton() {
  const [open, setOpen] = useState(false);
  const create = useCreateExperiment();
  const { is } = useRole();
  const navigate = useNavigate();
  const routes = useExperimentRoutes();

  const submit = async (config: ExperimentDesignInput) => {
    await create.mutateAsync(config);
    toast.success(`Created ${config.experiment} as a draft`);
    setOpen(false);
    navigate(routes.DETAIL(config.experiment));
  };

  return (
    <>
      <Button
        size='sm'
        disabled={!is.workspaceEditor}
        title={is.workspaceEditor ? undefined : "Viewers cannot create experiments."}
        onClick={() => setOpen(true)}
        data-testid='experiment-new'
      >
        <Plus className='h-4 w-4' />
        New experiment
      </Button>
      <DesignDialog
        open={open}
        onOpenChange={setOpen}
        title='New experiment'
        description='Describe the experiment. It is saved as a draft; pricing then proposes the schedule.'
        submitLabel='Create experiment'
        onSubmit={submit}
      />
    </>
  );
}
