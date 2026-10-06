import { useMutation, useQueryClient } from "@tanstack/react-query";
import { toast } from "sonner";
import { UserTokenService } from "@/services/api/apiToken";
import type {
  CreateSandboxAgentTokenRequest,
  CreateTokenRequest,
  Token,
  TokenWithSecret,
  UpdateTokenRequest
} from "@/types/apiToken";
import queryKeys from "../queryKey";
import { isStaleTokenError, isUnavailableAppError, tokenErrorMessage } from "./tokenErrors";

/** A personal token also shows in the inventory of every workspace it reaches: `apiKey.all`. */
const useInvalidateTokenLists = () => {
  const queryClient = useQueryClient();
  return () => {
    queryClient.invalidateQueries({ queryKey: queryKeys.userToken.all });
    queryClient.invalidateQueries({ queryKey: queryKeys.apiKey.all });
  };
};

/**
 * Create a token. The secret comes back once: the caller shows it from its own `onSuccess`, so
 * this hook never toasts or logs it.
 */
export const useCreateUserToken = () => {
  const invalidate = useInvalidateTokenLists();
  return useMutation<TokenWithSecret, Error, CreateTokenRequest>({
    mutationFn: (request) => UserTokenService.create(request),
    onSuccess: invalidate,
    onError: (error) => toast.error(tokenErrorMessage(error, "create"))
  });
};

/**
 * Mint a sandbox agent token. As with create, the caller shows the secret. It shows a refusal
 * too, beside the apps it is about, so this hook toasts nothing. A 404 `app_not_found` means
 * the apps on offer are out of date, so they are read again.
 */
export const useCreateSandboxAgentToken = () => {
  const queryClient = useQueryClient();
  const invalidate = useInvalidateTokenLists();
  return useMutation<TokenWithSecret, Error, CreateSandboxAgentTokenRequest>({
    mutationFn: (request) => UserTokenService.create(request),
    onSuccess: invalidate,
    onError: (error) => {
      if (isUnavailableAppError(error)) {
        queryClient.invalidateQueries({ queryKey: queryKeys.userToken.options() });
      }
    }
  });
};

export interface UpdateUserTokenVariables {
  id: string;
  request: UpdateTokenRequest;
}

export const useUpdateUserToken = () => {
  const invalidate = useInvalidateTokenLists();
  return useMutation<Token, Error, UpdateUserTokenVariables>({
    mutationFn: ({ id, request }) => UserTokenService.update(id, request),
    onSuccess: (updated) => {
      invalidate();
      toast.success(`Updated access for "${updated.name}"`);
    },
    onError: (error) => {
      if (isStaleTokenError(error)) invalidate();
      toast.error(tokenErrorMessage(error, "update"));
    }
  });
};

export interface RenameUserTokenVariables {
  id: string;
  name: string;
}

/**
 * Rename a token. The body is `{ name }` and nothing else, so its access is untouched.
 * The caller shows a refusal beside the box it was typed in, so this hook toasts only success.
 */
export const useRenameUserToken = () => {
  const invalidate = useInvalidateTokenLists();
  return useMutation<Token, Error, RenameUserTokenVariables>({
    mutationFn: ({ id, name }) => UserTokenService.update(id, { name }),
    onSuccess: (updated) => {
      invalidate();
      toast.success(`Renamed to "${updated.name}"`);
    },
    onError: (error) => {
      if (isStaleTokenError(error)) invalidate();
    }
  });
};

/** New secret, same id and grants. As with create, the caller shows the secret. */
export const useRegenerateUserToken = () => {
  const invalidate = useInvalidateTokenLists();
  return useMutation<TokenWithSecret, Error, string>({
    mutationFn: (id) => UserTokenService.regenerate(id),
    onSuccess: invalidate,
    onError: (error) => {
      if (isStaleTokenError(error)) invalidate();
      toast.error(tokenErrorMessage(error, "regenerate"));
    }
  });
};

export const useRevokeUserToken = () => {
  const invalidate = useInvalidateTokenLists();
  return useMutation<void, Error, Pick<Token, "id" | "name">>({
    mutationFn: ({ id }) => UserTokenService.revoke(id),
    onSuccess: (_data, { name }) => {
      invalidate();
      toast.success(`Revoked "${name}"`);
    },
    onError: (error) => {
      if (isStaleTokenError(error)) invalidate();
      toast.error(tokenErrorMessage(error, "revoke"));
    }
  });
};
