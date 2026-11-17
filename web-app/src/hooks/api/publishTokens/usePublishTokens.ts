import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { isAxiosError } from "axios";
import { toast } from "sonner";
import { PublishTokensService } from "@/services/api/publishTokens";
import type { CreatedPublishToken } from "@/types/publishTokens";
import queryKeys from "../queryKey";

function errorMessage(err: unknown, fallback: string): string {
  if (isAxiosError(err)) return err.response?.data?.message ?? err.message;
  if (err instanceof Error) return err.message;
  return fallback;
}

export const usePublishTokens = (options: { enabled?: boolean } = {}) =>
  useQuery({
    queryKey: queryKeys.publishTokens.list(),
    queryFn: PublishTokensService.list,
    enabled: options.enabled ?? true
  });

/**
 * Create a token. The plaintext is returned once — the caller (page) opens
 * a "copy it now" dialog in its own `onSuccess`; this hook only invalidates
 * the list and surfaces errors, so it never logs/toasts the secret.
 */
export const useCreatePublishToken = () => {
  const qc = useQueryClient();
  return useMutation<CreatedPublishToken, unknown, string>({
    mutationFn: (name: string) => PublishTokensService.create(name),
    onSuccess: () => {
      qc.invalidateQueries({ queryKey: queryKeys.publishTokens.list() });
    },
    onError: (err) => toast.error(errorMessage(err, "Failed to create token"))
  });
};

export const useRevokePublishToken = () => {
  const qc = useQueryClient();
  return useMutation({
    mutationFn: (id: string) => PublishTokensService.revoke(id),
    onSuccess: () => {
      qc.invalidateQueries({ queryKey: queryKeys.publishTokens.list() });
      toast.success("Token revoked");
    },
    onError: (err) => toast.error(errorMessage(err, "Failed to revoke token"))
  });
};
