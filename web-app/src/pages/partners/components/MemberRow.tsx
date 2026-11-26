import { toast } from "sonner";
import { DESIGN_REFERENCE } from "@/components/ui/reference/tokens";
import {
  AlertDialog,
  AlertDialogCancel,
  AlertDialogContent,
  AlertDialogDescription,
  AlertDialogDestructiveAction,
  AlertDialogFooter,
  AlertDialogHeader,
  AlertDialogTitle,
  AlertDialogTrigger
} from "@/components/ui/shadcn/alert-dialog";
import { Badge } from "@/components/ui/shadcn/badge";
import { Button } from "@/components/ui/shadcn/button";
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue
} from "@/components/ui/shadcn/select";
import { TableCell, TableRow } from "@/components/ui/shadcn/table";
import { useRemoveMember, useUpdateMemberRole } from "@/hooks/api/partners";
import type { PartnerOrgMember } from "@/types/partners";

/**
 * One member row. Owners are read-only — a partner can never change or remove an
 * Owner (the server enforces this too); Member/Admin rows get a role picker and
 * a confirm-to-remove action.
 */
export default function MemberRow({
  partnerId,
  orgId,
  member
}: {
  partnerId: string;
  orgId: string;
  member: PartnerOrgMember;
}) {
  const updateRole = useUpdateMemberRole(partnerId);
  const remove = useRemoveMember(partnerId);
  const isOwner = member.role === "owner";

  return (
    <TableRow data-testid={`partner-member-${member.user_id}`}>
      <TableCell className='font-medium'>{member.email}</TableCell>
      <TableCell className='text-muted-foreground'>{member.name ?? "—"}</TableCell>
      <TableCell>
        {isOwner ? (
          <Badge variant='secondary'>owner</Badge>
        ) : (
          <Select
            value={member.role}
            onValueChange={(role) =>
              updateRole.mutate(
                { orgId, userId: member.user_id, role },
                {
                  onSuccess: () => toast.success("Role updated"),
                  onError: () => toast.error("Failed to update role")
                }
              )
            }
          >
            <SelectTrigger className='w-28' data-testid={`partner-member-role-${member.user_id}`}>
              <SelectValue />
            </SelectTrigger>
            <SelectContent className={DESIGN_REFERENCE}>
              <SelectItem value='member' data-testid='partner-member-role-member'>
                Member
              </SelectItem>
              <SelectItem value='admin' data-testid='partner-member-role-admin'>
                Admin
              </SelectItem>
            </SelectContent>
          </Select>
        )}
      </TableCell>
      <TableCell className='text-right'>
        {!isOwner && (
          <AlertDialog>
            <AlertDialogTrigger asChild>
              <Button
                variant='ghost'
                size='sm'
                data-testid={`partner-member-remove-${member.user_id}`}
              >
                Remove
              </Button>
            </AlertDialogTrigger>
            <AlertDialogContent className={DESIGN_REFERENCE}>
              <AlertDialogHeader>
                <AlertDialogTitle>Remove {member.email}?</AlertDialogTitle>
                <AlertDialogDescription>
                  They'll lose access to this organization.
                </AlertDialogDescription>
              </AlertDialogHeader>
              <AlertDialogFooter>
                <AlertDialogCancel data-testid='partner-member-remove-cancel'>
                  Cancel
                </AlertDialogCancel>
                <AlertDialogDestructiveAction
                  data-testid='partner-member-remove-confirm'
                  onClick={() =>
                    remove.mutate(
                      { orgId, userId: member.user_id },
                      {
                        onSuccess: () => toast.success("Member removed"),
                        onError: () => toast.error("Failed to remove member")
                      }
                    )
                  }
                >
                  Remove
                </AlertDialogDestructiveAction>
              </AlertDialogFooter>
            </AlertDialogContent>
          </AlertDialog>
        )}
      </TableCell>
    </TableRow>
  );
}
