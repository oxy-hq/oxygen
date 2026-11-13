import { X } from "lucide-react";
import { Button } from "@/components/ui/shadcn/button";
import { Input } from "@/components/ui/shadcn/input";
import type { Variable } from "..";

interface VariableRowProps {
  variable: Variable;
  onUpdate: (updates: Partial<Variable>) => void;
  onRemove: () => void;
}

const VariableRow = ({ variable, onUpdate, onRemove }: VariableRowProps) => {
  return (
    <div className='flex items-center gap-2'>
      <Input
        type='text'
        value={variable.key}
        onChange={(e) => onUpdate({ key: e.target.value })}
        placeholder='Variable name'
        className='w-32'
      />
      <Input
        type='text'
        value={variable.value}
        onChange={(e) => onUpdate({ value: e.target.value })}
        placeholder='Value'
        className='flex-1'
      />
      <Button size='sm' variant='ghost' onClick={onRemove}>
        <X />
      </Button>
    </div>
  );
};

export default VariableRow;
