import ConfirmDialog from "@/components/settings/SettingsDialog/sections/account/PersonalTokens/components/TokenRow/components/ConfirmDialog";
import type { Token } from "@/types/apiToken";
import type { RevokeConfirmation } from "./useRevokeConfirmation";

interface Props {
  /** `admin-<area>`: the confirming button is `<area>-revoke-confirm`. */
  area: string;
  confirmation: RevokeConfirmation;
  /** Whose token it is and what stops, in the page's own words. */
  warning: (token: Token | null) => string;
}

/** A staff list's revoke confirmation. It names the token, and is the account list's own dialog. */
export function RevokeTokenConfirm({ area, confirmation, warning }: Props) {
  const { target } = confirmation;
  return (
    <ConfirmDialog
      open={confirmation.open}
      onOpenChange={confirmation.setOpen}
      title={`Revoke ${target?.name ?? "this token"}?`}
      description={warning(target)}
      action='Revoke'
      testId={`${area}-revoke-confirm`}
      onConfirm={confirmation.confirm}
    />
  );
}
