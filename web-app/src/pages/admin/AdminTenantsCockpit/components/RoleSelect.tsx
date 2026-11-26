import { DESIGN_REFERENCE } from "@/components/ui/reference/tokens";
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue
} from "@/components/ui/shadcn/select";
import type { OrgRoleId } from "@/services/api/adminTenants";

/** Inline org-role editor (owner / admin / member). */
export default function RoleSelect({
  value,
  onChange
}: {
  value: string;
  onChange: (r: OrgRoleId) => void;
}) {
  return (
    <Select value={value} onValueChange={(v) => onChange(v as OrgRoleId)}>
      <SelectTrigger className='h-8 w-28'>
        <SelectValue />
      </SelectTrigger>
      <SelectContent className={DESIGN_REFERENCE}>
        <SelectItem value='owner'>Owner</SelectItem>
        <SelectItem value='admin'>Admin</SelectItem>
        <SelectItem value='member'>Member</SelectItem>
      </SelectContent>
    </Select>
  );
}
