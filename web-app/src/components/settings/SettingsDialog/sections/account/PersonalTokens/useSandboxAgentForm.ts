import { useEffect, useState } from "react";
import { sandboxMintErrorMessage } from "@/hooks/api/userTokens/tokenErrors";
import { useCreateSandboxAgentToken } from "@/hooks/api/userTokens/useUserTokenMutations";
import { sandboxApps, sandboxLimits } from "@/libs/sandboxAgentToken";
import type { TokenOptions, TokenWithSecret } from "@/types/apiToken";
import {
  effectiveLifetime,
  emptySandboxDraft,
  type LifetimeChoice,
  pickedApps,
  type SandboxDraft,
  sandboxMintRequest,
  toggleApp
} from "./sandboxDraft";

/**
 * The sandbox agent half of the create dialog: whether the type is on offer and chosen, the apps
 * and lifetime picked, and the mint itself. The dialog keeps the name, which both types share.
 *
 * `offered` is false until the options name at least one app, so the type is never shown to
 * someone who can't mint one. A refusal is kept here to be shown beside the picks it is about,
 * and is cleared by the next edit.
 */
const useSandboxAgentForm = (open: boolean, name: string, options: TokenOptions | undefined) => {
  const [chosen, setChosen] = useState(false);
  const [draft, setDraft] = useState<SandboxDraft>(emptySandboxDraft);
  const [refusal, setRefusal] = useState<string | null>(null);
  const mint = useCreateSandboxAgentToken();

  // A fresh form each time it opens, as for the personal token fields.
  useEffect(() => {
    if (!open) return;
    setChosen(false);
    setDraft(emptySandboxDraft());
    setRefusal(null);
  }, [open]);

  const apps = sandboxApps(options);
  const limits = sandboxLimits(options);
  const lifetime = effectiveLifetime(draft, limits);
  const picked = pickedApps(draft.appIds, apps);
  const request = sandboxMintRequest(name, picked, lifetime, limits);

  return {
    offered: apps.length > 0,
    active: chosen && apps.length > 0,
    setActive: setChosen,
    apps,
    limits,
    picked,
    lifetime,
    refusal,
    isPending: mint.isPending,
    canSubmit: !!request && !mint.isPending,
    toggle: (id: string, on: boolean) => {
      setRefusal(null);
      // From the picks still on offer, so an app that dropped out doesn't count against the limit.
      const current = picked.map((app) => app.id);
      setDraft((before) => ({ ...before, appIds: toggleApp(current, id, on, limits) }));
    },
    setLifetime: (choice: LifetimeChoice) => {
      setRefusal(null);
      setDraft((before) => ({ ...before, lifetime: choice }));
    },
    /** Handed the one response that carries the secret. */
    submit: (onCreated: (created: TokenWithSecret) => void) => {
      if (!request || mint.isPending) return;
      setRefusal(null);
      mint.mutate(request, {
        onSuccess: onCreated,
        onError: (error) => setRefusal(sandboxMintErrorMessage(error, apps, limits))
      });
    }
  };
};

export type SandboxAgentForm = ReturnType<typeof useSandboxAgentForm>;

export default useSandboxAgentForm;
