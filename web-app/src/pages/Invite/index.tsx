import { useEffect, useState } from "react";
import { useNavigate, useParams } from "react-router-dom";
import { useAuth } from "@/contexts/AuthContext";
import { PENDING_INVITE_TOKEN_KEY } from "@/hooks/auth/postLoginRedirect";
import ROUTES from "@/libs/utils/routes";
import { ErrorCard } from "./components/ErrorCard";
import { PendingCard } from "./components/PendingCard";
import { SuccessCard } from "./components/SuccessCard";
import { errorContent } from "./errorContent";
import type { InviteStatus } from "./types";
import { useAcceptInviteFlow } from "./useAcceptInviteFlow";

type InviteState =
  | { kind: "pending" }
  | { kind: "error"; status: InviteStatus }
  | { kind: "success"; slug: string };

export default function InvitePage() {
  const { token } = useParams<{ token: string }>();
  const navigate = useNavigate();
  const { isAuthenticated, authConfig, logout } = useAuth();
  const [state, setState] = useState<InviteState>({ kind: "pending" });

  const needsLogin = !!token && authConfig.auth_enabled && !isAuthenticated();
  useEffect(() => {
    if (!needsLogin || !token) return;
    sessionStorage.setItem(PENDING_INVITE_TOKEN_KEY, token);
    navigate(ROUTES.AUTH.LOGIN, { replace: true });
  }, [needsLogin, token, navigate]);

  const retry = useAcceptInviteFlow({
    token,
    enabled: !!token && !needsLogin,
    onSuccess: (slug) => setState({ kind: "success", slug }),
    onError: (status) => {
      if (status === 401 && token) {
        sessionStorage.setItem(PENDING_INVITE_TOKEN_KEY, token);
      }
      setState({ kind: "error", status });
    },
    onReset: () => setState({ kind: "pending" })
  });

  if (!token) {
    return (
      <ErrorCard
        title='Invalid invitation link'
        description='This invitation link is missing or malformed.'
      />
    );
  }

  if (state.kind === "success") {
    return <SuccessCard onDone={() => navigate(ROUTES.ORG(state.slug).ROOT, { replace: true })} />;
  }

  if (state.kind === "error") {
    return (
      <ErrorCard
        {...errorContent(state.status, {
          token,
          retry,
          signIn: () => {
            void navigate(ROUTES.AUTH.LOGIN, { replace: true });
          },
          signOut: logout
        })}
      />
    );
  }

  return <PendingCard />;
}
