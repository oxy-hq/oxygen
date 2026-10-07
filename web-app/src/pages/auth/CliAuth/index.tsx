import type React from "react";
import { useMemo } from "react";
import { useSearchParams } from "react-router-dom";
import { parseCliAuthRequest } from "./cliAuthRequest";
import CliAuthCard from "./components/CliAuthCard";
import LegacyHandoff from "./components/LegacyHandoff";
import MintAuthorize from "./components/MintAuthorize";
import MintPage from "./components/MintPage";
import PkceAuthorize from "./components/PkceAuthorize";

/**
 * Whether another page is showing this one in a frame. Comparing the two windows is allowed
 * across origins; if even that throws, treat the page as framed.
 */
export const isFramed = (): boolean => {
  try {
    return window.top !== window.self;
  } catch {
    return true;
  }
};

/**
 * `/cli-auth` — the browser side of `oxyc login`, and of `oxyc tokens create --sandbox-agent`
 * and `oxyc tokens create --agent`.
 *
 * A current oxyc opens this page with `?port&state&code_challenge&hostname` (PKCE): the person
 * confirms the computer, and oxyc gets a single-use code to exchange for its own token. An older
 * oxyc sends only `?port&state` and is handed the session token, once the person agrees to it.
 *
 * With a `kind` beside the PKCE params, the page approves a token instead: the same handoff, and
 * the code yields that token. `kind=sandbox_agent` with `apps`, `hours` and `name` is a sandbox
 * agent token; `kind=agent` with `hours`, `standing` and `name` is an agent token, which reaches
 * everything its approver does. A link that asks for a kind the page doesn't know, or for both,
 * is refused on the same sheet, with nothing on it to approve.
 *
 * Security: the callback URL is only ever `http://127.0.0.1:<port>` built from the integer
 * `port` param — never an arbitrary URL — so this can't be abused as an open redirect.
 *
 * Every state here is a person agreeing to hand a credential to a program, so the page refuses
 * to run inside another page: a frame lets the outer page cover the card and steer a click onto
 * the button. Nothing is asked and nothing is sent while framed.
 */
const CliAuth: React.FC = () => {
  const [params] = useSearchParams();
  const request = useMemo(() => parseCliAuthRequest(params), [params]);

  if (isFramed()) {
    return (
      <CliAuthCard
        status='error'
        title='Open this page in its own tab'
        description='This page asks you to approve a sign-in, so it does not work inside another page. Nothing was sent.'
      />
    );
  }

  switch (request.kind) {
    case "pkce":
      return <PkceAuthorize request={request} />;
    case "mint":
    case "agent_mint":
      return <MintAuthorize request={request} />;
    case "legacy":
      return <LegacyHandoff request={request} />;
    case "invalid":
      // A token request that can't be read is refused where one would have been approved.
      return request.title ? (
        <MintPage status='error' title={request.title} lead={request.reason} />
      ) : (
        <CliAuthCard status='error' title='CLI login failed' description={request.reason} />
      );
  }
};

export default CliAuth;
