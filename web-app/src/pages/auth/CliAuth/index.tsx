import type React from "react";
import { useMemo } from "react";
import { useSearchParams } from "react-router-dom";
import { parseCliAuthRequest } from "./cliAuthRequest";
import CliAuthCard from "./components/CliAuthCard";
import LegacyHandoff from "./components/LegacyHandoff";
import MintAuthorize from "./components/MintAuthorize";
import PkceAuthorize from "./components/PkceAuthorize";

/**
 * `/cli-auth` — the browser side of `oxyc login`, and of `oxyc tokens create --sandbox-agent`.
 *
 * A current oxyc opens this page with `?port&state&code_challenge&hostname` (PKCE): the person
 * confirms the computer, and oxyc gets a single-use code to exchange for its own token. An older
 * oxyc sends only `?port&state` and is handed the session token, exactly as before.
 *
 * With `kind=sandbox_agent` and `apps`, `hours` and `name` beside the PKCE params, the page
 * approves a sandbox agent token instead: the same handoff, and the code yields that token.
 *
 * Security: the callback URL is only ever `http://127.0.0.1:<port>` built from the integer
 * `port` param — never an arbitrary URL — so this can't be abused as an open redirect.
 */
const CliAuth: React.FC = () => {
  const [params] = useSearchParams();
  const request = useMemo(() => parseCliAuthRequest(params), [params]);

  switch (request.kind) {
    case "pkce":
      return <PkceAuthorize request={request} />;
    case "mint":
      return <MintAuthorize request={request} />;
    case "legacy":
      return <LegacyHandoff request={request} />;
    case "invalid":
      return (
        <CliAuthCard
          status='error'
          title={request.title ?? "CLI login failed"}
          description={request.reason}
        />
      );
  }
};

export default CliAuth;
