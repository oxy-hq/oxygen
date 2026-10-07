import type React from "react";

interface Props {
  hostname: string;
  /** Who the token will act as. */
  email?: string;
}

/**
 * The two things a person checks first on any token request: the computer asking, and who the
 * agent acts as. Both wrap as whole words, so neither is split in the middle of a name.
 */
const MintLead: React.FC<Props> = ({ hostname, email }) => (
  <>
    Asked from the computer{" "}
    <b className='break-words font-medium text-foreground' data-testid='cli-auth-hostname'>
      {hostname}
    </b>
    , for an agent that will act as{" "}
    <b className='break-words font-medium text-foreground' data-testid='cli-auth-approver'>
      {email ?? "you"}
    </b>
    .
  </>
);

export default MintLead;
