import { useMutation, useQueryClient } from "@tanstack/react-query";
import { CliAuthService } from "@/services/api/apiToken";
import type { CliAuthorizeRequest, CliAuthorizeResponse } from "@/types/apiToken";
import queryKeys from "../queryKey";
import { isUnavailableAppError } from "../userTokens/tokenErrors";

/**
 * Trade the browser session for a single-use code oxyc can exchange (PKCE): a login, or with
 * `mint` a sandbox agent token. No toast: the `/cli-auth` page is nothing but this request, so
 * it renders the outcome itself.
 *
 * A mint refused with 404 `app_not_found` means the apps the page resolved against are out of
 * date, so they are read again and the approval shows that app as one it can't mint for.
 */
const useAuthorizeCli = () => {
  const queryClient = useQueryClient();
  return useMutation<CliAuthorizeResponse, Error, CliAuthorizeRequest>({
    mutationFn: (request) => CliAuthService.authorize(request),
    onError: (error) => {
      if (isUnavailableAppError(error)) {
        queryClient.invalidateQueries({ queryKey: queryKeys.userToken.options() });
      }
    }
  });
};

export default useAuthorizeCli;
