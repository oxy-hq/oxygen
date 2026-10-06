import { MoreHorizontal } from "lucide-react";
import type React from "react";
import { useState } from "react";
import {
  AlertDialog,
  AlertDialogAction,
  AlertDialogCancel,
  AlertDialogContent,
  AlertDialogDescription,
  AlertDialogFooter,
  AlertDialogHeader,
  AlertDialogTitle
} from "@/components/ui/shadcn/alert-dialog";
import { Button } from "@/components/ui/shadcn/button";
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuItem,
  DropdownMenuSeparator,
  DropdownMenuTrigger
} from "@/components/ui/shadcn/dropdown-menu";
import { buttonVariants } from "@/components/ui/shadcn/utils/button-variants";
import {
  useRegenerateUserToken,
  useRevokeUserToken
} from "@/hooks/api/userTokens/useUserTokenMutations";
import type { Token, TokenWithSecret } from "@/types/apiToken";
import EditAccessDialog from "../../EditAccessDialog";

interface Props {
  token: Token;
  /** Handed the one response that carries the new secret. */
  onRegenerated: (regenerated: TokenWithSecret) => void;
}

interface ConfirmProps {
  open: boolean;
  onOpenChange: (open: boolean) => void;
  title: string;
  description: string;
  action: string;
  testId: string;
  onConfirm: () => void;
}

/** Both destructive actions ask first: each one stops a deployed secret from working. */
const ConfirmDialog: React.FC<ConfirmProps> = ({
  open,
  onOpenChange,
  title,
  description,
  action,
  testId,
  onConfirm
}) => (
  <AlertDialog open={open} onOpenChange={onOpenChange}>
    <AlertDialogContent className='bg-popover sm:max-w-md'>
      <AlertDialogHeader>
        <AlertDialogTitle>{title}</AlertDialogTitle>
        <AlertDialogDescription>{description}</AlertDialogDescription>
      </AlertDialogHeader>
      <AlertDialogFooter>
        <AlertDialogCancel>Cancel</AlertDialogCancel>
        <AlertDialogAction
          onClick={onConfirm}
          className={buttonVariants({ variant: "destructive" })}
          data-testid={testId}
        >
          {action}
        </AlertDialogAction>
      </AlertDialogFooter>
    </AlertDialogContent>
  </AlertDialog>
);

/** Edit access, Regenerate and Revoke for one personal access token. */
const TokenRowMenu: React.FC<Props> = ({ token, onRegenerated }) => {
  const [editOpen, setEditOpen] = useState(false);
  const [regenerateOpen, setRegenerateOpen] = useState(false);
  const [revokeOpen, setRevokeOpen] = useState(false);
  const regenerate = useRegenerateUserToken();
  const revoke = useRevokeUserToken();

  return (
    <>
      {/* `modal={false}`: a modal menu that opens a dialog leaves `pointer-events: none` on body. */}
      <DropdownMenu modal={false}>
        <DropdownMenuTrigger asChild>
          <Button
            variant='ghost'
            size='sm'
            className='data-[state=open]:bg-muted'
            disabled={regenerate.isPending || revoke.isPending}
            aria-label={`More actions for ${token.name}`}
            data-testid='account-token-menu-button'
          >
            <MoreHorizontal />
          </Button>
        </DropdownMenuTrigger>
        <DropdownMenuContent align='end' className='w-44'>
          <DropdownMenuItem
            onClick={() => setEditOpen(true)}
            data-testid='account-token-edit-access'
          >
            Edit access…
          </DropdownMenuItem>
          <DropdownMenuItem
            onClick={() => setRegenerateOpen(true)}
            data-testid='account-token-regenerate'
          >
            Regenerate…
          </DropdownMenuItem>
          <DropdownMenuSeparator />
          <DropdownMenuItem
            className='text-destructive focus:text-destructive'
            onClick={() => setRevokeOpen(true)}
            data-testid='account-token-revoke'
          >
            Revoke…
          </DropdownMenuItem>
        </DropdownMenuContent>
      </DropdownMenu>

      <EditAccessDialog token={token} open={editOpen} onOpenChange={setEditOpen} />

      <ConfirmDialog
        open={regenerateOpen}
        onOpenChange={setRegenerateOpen}
        title={`Regenerate ${token.name}?`}
        description='The current secret stops working at once. You get a new one with the same access and expiry, to put wherever this token is used.'
        action='Regenerate'
        testId='account-token-regenerate-confirm'
        onConfirm={() => regenerate.mutate(token.id, { onSuccess: onRegenerated })}
      />
      <ConfirmDialog
        open={revokeOpen}
        onOpenChange={setRevokeOpen}
        title={`Revoke ${token.name}?`}
        description="Anything using it stops working at once, and it can't be brought back."
        action='Revoke'
        testId='account-token-revoke-confirm'
        onConfirm={() => revoke.mutate({ id: token.id, name: token.name })}
      />
    </>
  );
};

export default TokenRowMenu;
