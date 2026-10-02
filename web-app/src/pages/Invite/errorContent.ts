import { PENDING_INVITE_TOKEN_KEY } from "@/hooks/auth/postLoginRedirect";
import type { ErrorAction, InviteStatus } from "./types";

export type ErrorContent = {
  title: string;
  description: string;
  primaryAction?: ErrorAction;
};

export type ErrorContext = {
  token: string;
  retry: () => void;
  signIn: () => void;
  signOut: () => Promise<void>;
};

export function errorContent(status: InviteStatus, ctx: ErrorContext): ErrorContent {
  if (status === 401) {
    return {
      title: "Session expired",
      description: "Please sign in again to accept your invitation.",
      primaryAction: { label: "Sign in", onClick: ctx.signIn }
    };
  }

  if (status === 403) {
    return {
      title: "Wrong account",
      description:
        "This invitation was sent to a different email. Sign out and sign in with the invited address to accept it.",
      primaryAction: {
        label: "Sign out",
        onClick: () => {
          // Sign-out ends by clearing sessionStorage, so the pending-invite token is
          // stored only once it has finished — any earlier and it is wiped, and signing
          // in with the invited address never comes back to this invitation.
          ctx
            .signOut()
            .then(() => sessionStorage.setItem(PENDING_INVITE_TOKEN_KEY, ctx.token))
            .catch((error: unknown) => console.error("Sign out failed", error));
        }
      }
    };
  }

  if (status === "network") {
    return {
      title: "Connection error",
      description: "We couldn't reach the server. Check your connection and try again.",
      primaryAction: { label: "Try again", onClick: ctx.retry }
    };
  }

  if (typeof status === "number" && status >= 500) {
    return {
      title: "Something went wrong",
      description: "The server ran into an error. Please try again in a moment.",
      primaryAction: { label: "Try again", onClick: ctx.retry }
    };
  }

  return {
    title: "Invitation unavailable",
    description: descriptionForClientError(status)
  };
}

function descriptionForClientError(status: InviteStatus): string {
  switch (status) {
    case 404:
      return "This invitation link is invalid or has been revoked.";
    case 409:
      return "You're already a member of this organization.";
    case 400:
      return "This invitation has expired or is no longer valid.";
    default:
      return "This invitation is invalid or has expired.";
  }
}
