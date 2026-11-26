import { useState } from "react";
import { toast } from "sonner";
import { DESIGN_REFERENCE } from "@/components/ui/reference/tokens";
import { Button } from "@/components/ui/shadcn/button";
import { Input } from "@/components/ui/shadcn/input";
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue
} from "@/components/ui/shadcn/select";
import { useInviteMember } from "@/hooks/api/partners";

/** Invite a member to a managed org. Partners can only assign Member/Admin. */
export default function InviteMemberForm({
  partnerId,
  orgId
}: {
  partnerId: string;
  orgId: string;
}) {
  const invite = useInviteMember(partnerId);
  const [email, setEmail] = useState("");
  const [role, setRole] = useState("member");

  const submit = () => {
    const trimmed = email.trim();
    if (!trimmed) return;
    invite.mutate(
      { orgId, email: trimmed, role },
      {
        onSuccess: () => {
          toast.success(`Invited ${trimmed}`);
          setEmail("");
        },
        onError: () => toast.error("Failed to invite member")
      }
    );
  };

  return (
    <div className='flex flex-wrap items-center gap-2'>
      <Input
        type='email'
        placeholder='teammate@company.com'
        value={email}
        onChange={(e) => setEmail(e.target.value)}
        className='max-w-xs'
        data-testid='partner-invite-email'
      />
      <Select value={role} onValueChange={setRole}>
        <SelectTrigger className='w-32' data-testid='partner-invite-role'>
          <SelectValue />
        </SelectTrigger>
        <SelectContent className={DESIGN_REFERENCE}>
          <SelectItem value='member' data-testid='partner-invite-role-member'>
            Member
          </SelectItem>
          <SelectItem value='admin' data-testid='partner-invite-role-admin'>
            Admin
          </SelectItem>
        </SelectContent>
      </Select>
      <Button
        onClick={submit}
        disabled={invite.isPending || !email.trim()}
        data-testid='partner-invite-submit'
      >
        Invite
      </Button>
    </div>
  );
}
