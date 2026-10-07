import { type QueryKey, useMutation, useQueryClient } from "@tanstack/react-query";
import { toast } from "sonner";
import { apiStatus } from "@/libs/apiError";
import type { Token } from "@/types/apiToken";
import queryKeys from "../queryKey";

/**
 * The retry rule of a staff token list. A 403 is the capability gate: asking again cannot change
 * it, so it is not retried.
 */
export const retryUnlessRefused = (failureCount: number, error: unknown): boolean =>
  apiStatus(error) !== 403 && failureCount < 3;

/**
 * What a failed revoke says. `null` for a 403: the API client already said "You don't have
 * permission to do this." for it. A 404 is said in the list's own words, since what it can mean
 * differs from one list to the next.
 */
export const listedRevokeError = (error: unknown, notFound: string): string | null => {
  switch (apiStatus(error)) {
    case 403:
      return null;
    case 404:
      return notFound;
    default:
      return "Couldn't revoke the token. Try again.";
  }
};

interface Options {
  revoke: (id: string) => Promise<Token>;
  /** The staff list the token is in. */
  listKey: QueryKey;
  errorMessage: (error: unknown) => string | null;
}

/**
 * Revoke one token from a staff list, whoever owns it. The list is read again whether it worked
 * or not: a refusal means the row on screen was stale. The owner's own list in Settings is read
 * again too.
 */
export const useRevokeListedToken = ({ revoke, listKey, errorMessage }: Options) => {
  const queryClient = useQueryClient();
  return useMutation<Token, unknown, Pick<Token, "id" | "name">>({
    mutationFn: ({ id }) => revoke(id),
    onSuccess: (_token, { name }) => {
      toast.success(`Revoked "${name}"`);
    },
    onError: (error) => {
      const message = errorMessage(error);
      if (message) toast.error(message);
    },
    onSettled: () => {
      queryClient.invalidateQueries({ queryKey: listKey });
      queryClient.invalidateQueries({ queryKey: queryKeys.userToken.all });
    }
  });
};
