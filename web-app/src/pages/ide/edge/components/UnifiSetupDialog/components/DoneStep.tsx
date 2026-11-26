import { CheckCircle2 } from "lucide-react";
import type React from "react";
import { Button } from "@/components/ui/shadcn/button";
import type { UnifiImportResult } from "@/services/api";

type Props = {
  result: UnifiImportResult;
  onClose: () => void;
};

const DoneStep: React.FC<Props> = ({ result, onClose }) => (
  <div className='flex flex-col gap-4'>
    <div className='flex items-center gap-3'>
      <CheckCircle2 className='size-6 text-primary' />
      <div className='flex flex-col'>
        <p className='font-medium'>UniFi import complete</p>
        <p className='text-muted-foreground text-sm'>
          Your sites and cameras are now in this workspace.
        </p>
      </div>
    </div>

    <dl className='grid grid-cols-3 gap-4 bg-muted/40 p-4 text-center'>
      <div>
        <dt className='text-muted-foreground text-xs'>Sites</dt>
        <dd className='font-semibold text-2xl'>{result.sites_upserted}</dd>
      </div>
      <div>
        <dt className='text-muted-foreground text-xs'>Edge boxes</dt>
        <dd className='font-semibold text-2xl'>{result.edge_boxes_upserted}</dd>
      </div>
      <div>
        <dt className='text-muted-foreground text-xs'>Cameras</dt>
        <dd className='font-semibold text-2xl'>{result.cameras_upserted}</dd>
      </div>
    </dl>

    {result.skipped_no_workspace > 0 && (
      <p className='text-muted-foreground text-xs'>
        {result.skipped_no_workspace} entries were skipped because they couldn't be attributed to a
        workspace.
      </p>
    )}

    <div className='flex justify-end'>
      <Button onClick={onClose}>Done</Button>
    </div>
  </div>
);

export default DoneStep;
