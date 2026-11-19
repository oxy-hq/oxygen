import type React from "react";
import { useId, useState } from "react";
import { Input } from "@/components/ui/shadcn/input";
import { cleanUserCode } from "../Device/deviceRequest";

interface Props {
  /** `BCDF-GHJK`: the code the request is held under. Never shown: the person types it. */
  code: string;
  /** Whether what is typed is that code. */
  onMatchedChange: (matched: boolean) => void;
  disabled?: boolean;
  /**
   * What approving hands to whoever started the request, for an approval that hands them no
   * token (a trust policy's). Defaults to the token's warning.
   */
  caution?: string;
}

/**
 * The step a device-code approval adds (`oxyc … --no-browser`): the person types the code oxyc
 * printed in their terminal, and Approve stays off until it is the code of this request.
 *
 * The page never shows the code. A request is collected by whoever started it, and a link that
 * shows its own code is a link anyone can forward with "just click Approve" (RFC 8628 §5.4);
 * typing it means reading it off the terminal that asked. That is also why a device-code token
 * never carries staff or partner access: the server holds to it whatever is approved here.
 */
const TOKEN_CAUTION =
  "Approve only a request you started yourself. Whoever started it gets the token.";

const DeviceCodeCheck: React.FC<Props> = ({
  code,
  onMatchedChange,
  disabled,
  caution = TOKEN_CAUTION
}) => {
  const id = useId();
  const [typed, setTyped] = useState("");
  const change = (value: string) => {
    setTyped(value);
    onMatchedChange(cleanUserCode(value) === code);
  };
  return (
    <div className='mt-2 text-sm leading-5.5' data-testid='cli-auth-device-code-check'>
      <p className='font-medium'>{caution}</p>
      <label htmlFor={id} className='mt-2 block'>
        Type the code oxyc printed in your terminal:
      </label>
      <Input
        id={id}
        autoComplete='off'
        autoCapitalize='characters'
        className='mt-2 max-w-60 font-mono uppercase tracking-widest'
        placeholder='BCDF-GHJK'
        value={typed}
        onChange={(event) => change(event.target.value)}
        disabled={disabled}
        data-testid='cli-auth-device-code-typed'
      />
    </div>
  );
};

export default DeviceCodeCheck;
