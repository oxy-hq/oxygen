import type React from "react";
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue
} from "@/components/ui/shadcn/select";
import type { RoleCeiling } from "@/types/apiToken";
import { CEILING_LABELS, CEILINGS } from "../../../accessSummary";

interface Props {
  value: RoleCeiling;
  onChange: (ceiling: RoleCeiling) => void;
  /** What the ceiling applies to, for screen readers: "Access to Analytics". */
  label: string;
  testId: string;
}

/** Read, Write, Admin or Full: the most a token may do through one grant. */
const CeilingSelect: React.FC<Props> = ({ value, onChange, label, testId }) => (
  <Select value={value} onValueChange={(next) => onChange(next as RoleCeiling)}>
    <SelectTrigger size='sm' className='w-24 text-xs' aria-label={label} data-testid={testId}>
      <SelectValue />
    </SelectTrigger>
    <SelectContent>
      {CEILINGS.map((ceiling) => (
        <SelectItem
          key={ceiling}
          value={ceiling}
          className='text-xs'
          data-testid={`${testId}-${ceiling}`}
        >
          {CEILING_LABELS[ceiling]}
        </SelectItem>
      ))}
    </SelectContent>
  </Select>
);

export default CeilingSelect;
