import { useMutation } from "@tanstack/react-query";
import { CliAuthService } from "@/services/api/apiToken";
import type { CliAuthorizeRequest, CliAuthorizeResponse } from "@/types/apiToken";

/**
 * Trade the browser session for a single-use code oxyc can exchange (PKCE). No toast: the
 * `/cli-auth` page is nothing but this request, so it renders the outcome itself.
 */
const useAuthorizeCli = () =>
  useMutation<CliAuthorizeResponse, Error, CliAuthorizeRequest>({
    mutationFn: (request) => CliAuthService.authorize(request)
  });

export default useAuthorizeCli;
