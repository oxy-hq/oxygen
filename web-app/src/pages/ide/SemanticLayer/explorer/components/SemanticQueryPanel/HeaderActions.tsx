import { Play } from "lucide-react";
import { Button } from "@/components/ui/shadcn/button";
import { Spinner } from "@/components/ui/shadcn/spinner";
import { Tooltip, TooltipContent, TooltipTrigger } from "@/components/ui/shadcn/tooltip";

interface HeaderActionsProps {
  onExecuteQuery: () => void;
  loading: boolean;
  disabled?: boolean;
  disabledMessage?: string;
}

const HeaderActions = ({
  onExecuteQuery,
  loading,
  disabled,
  disabledMessage
}: HeaderActionsProps) => {
  return (
    <div className='flex items-center gap-2 overflow-x-auto whitespace-nowrap'>
      <Tooltip>
        <TooltipTrigger asChild>
          <span>
            <Button
              size='sm'
              className='flex-shrink-0 hover:text-muted-foreground'
              variant='ghost'
              disabled={loading || disabled}
              onClick={onExecuteQuery}
              title='Run query'
              data-testid='semantic-run-query'
            >
              {loading ? <Spinner /> : <Play className='h-4 w-4' />}
            </Button>
          </span>
        </TooltipTrigger>
        <TooltipContent>
          {disabled ? disabledMessage || "Select dimensions or measures to run query" : "Run query"}
        </TooltipContent>
      </Tooltip>
    </div>
  );
};

export default HeaderActions;
