import { useMutation, useQueryClient } from "@tanstack/react-query";
import { toast } from "sonner";
import useCurrentProjectBranch from "@/hooks/useCurrentProjectBranch";
import { apiErrorMessage } from "@/libs/apiError";
import { SecretService } from "@/services/secretService";
import type {
  CreateSecretRequest,
  CreateSecretResponse,
  Secret,
  UpdateSecretRequest
} from "@/types/secret";
import queryKeys from "../queryKey";

/**
 * Creates one secret and says so. A caller that creates several for one submit
 * passes `toastOnSuccess: false` and reports them together, so the user gets one
 * toast for one action; a failure is always reported here, naming the secret
 * and saying why in the server's words ("Secret with name 'X' already exists").
 */
export const useCreateSecret = ({ toastOnSuccess = true }: { toastOnSuccess?: boolean } = {}) => {
  const queryClient = useQueryClient();
  const { project } = useCurrentProjectBranch();
  const projectId = project.id;

  return useMutation<CreateSecretResponse, Error, CreateSecretRequest>({
    mutationFn: (request) => SecretService.createSecret(projectId, request),
    onSuccess: () => {
      queryClient.invalidateQueries({
        queryKey: queryKeys.secret.list(projectId)
      });
      if (toastOnSuccess) toast.success("Secret created successfully");
    },
    onError: (error, request) => {
      console.error("Failed to create secret:", error);
      // With no reason from the server, axios's own message ("Request failed
      // with status code 502", "Network Error") is the one there is.
      toast.error(`Failed to create secret ${request.name}`, {
        description: apiErrorMessage(error, error.message)
      });
    }
  });
};

export const useUpdateSecret = () => {
  const queryClient = useQueryClient();
  const { project } = useCurrentProjectBranch();
  const projectId = project.id;

  return useMutation<Secret, Error, { id: string; request: UpdateSecretRequest }>({
    mutationFn: ({ id, request }) => SecretService.updateSecret(projectId, id, request),
    onSuccess: (data) => {
      queryClient.invalidateQueries({
        queryKey: queryKeys.secret.list(projectId)
      });
      // Update the specific secret in cache
      queryClient.invalidateQueries({
        queryKey: queryKeys.secret.item(projectId, data.id)
      });
      toast.success("Secret updated successfully");
    },
    onError: (error) => {
      console.error("Failed to update secret:", error);
      toast.error("Failed to update secret");
    }
  });
};

export const useDeleteSecret = () => {
  const queryClient = useQueryClient();
  const { project } = useCurrentProjectBranch();
  const projectId = project.id;

  return useMutation<void, Error, string>({
    mutationFn: (id) => SecretService.deleteSecret(projectId, id),
    onSuccess: () => {
      queryClient.invalidateQueries({
        queryKey: queryKeys.secret.list(projectId)
      });
      toast.success("Secret deleted successfully");
    },
    onError: (error) => {
      console.error("Failed to delete secret:", error);
      toast.error("Failed to delete secret");
    }
  });
};
