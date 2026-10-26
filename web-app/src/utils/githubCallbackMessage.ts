import { apiErrorCode, apiErrorMessage, apiStatus } from "@/libs/apiError";
import type { GitHubCallbackResponse } from "@/types/github";

type SuccessMessage = { type: "github-callback-success" } & GitHubCallbackResponse;
/** `code` is the server's machine-readable refusal code, when it sent one. */
type ErrorMessage = { type: "github-callback-error"; reason: string; code?: string };
export type CallbackMessage = SuccessMessage | ErrorMessage;

export class GitHubCallbackCancelled extends Error {
  constructor() {
    super("Cancelled");
    this.name = "GitHubCallbackCancelled";
  }
}

/** The popup reported a failure. `message` is written for a person. */
export class GitHubCallbackError extends Error {
  readonly code?: string;

  constructor(reason: string, code?: string) {
    super(reason);
    this.name = "GitHubCallbackError";
    this.code = code;
  }
}

/**
 * What a bare refusal from `POST /user/github/callback` means. These routes answer
 * a status with no body, and axios's own message ("Request failed with status code
 * 404") is all the person would otherwise see.
 */
function callbackRefusal(status: number | undefined): string {
  switch (status) {
    case 400:
      return "This GitHub link has expired or is no longer valid. Start again from Oxygen.";
    case 403:
      return "Only an organization owner or admin can connect a GitHub App installation, and only the person who started the connection can finish it.";
    case 404:
      return "That GitHub installation isn't one your GitHub account can access.";
    case 409:
      return "Reconnect your GitHub account, then try again.";
    default:
      return "Couldn't finish connecting GitHub. Try again.";
  }
}

/** The failure of the callback request, as the popup reports it to its opener. */
export function describeCallbackFailure(err: unknown): { reason: string; code?: string } {
  return {
    reason: apiErrorMessage(err, callbackRefusal(apiStatus(err))),
    code: apiErrorCode(err)
  };
}

const POPUP_CLOSED_POLL_MS = 500;

/**
 * Listens for a postMessage from the popup at /github/callback.
 * Resolves with the success payload (filtered by `expectedFlow`) or rejects
 * if the popup posts an error, the popup is closed before posting, or the
 * popup is null (blocked).
 */
export function waitForGitHubCallback<F extends GitHubCallbackResponse["flow"]>(
  popup: Window | null,
  expectedFlow: F
): Promise<Extract<GitHubCallbackResponse, { flow: F }>> {
  return new Promise((resolve, reject) => {
    if (!popup) {
      reject(new Error("Popup blocked. Please enable popups for this site."));
      return;
    }

    let settled = false;
    const cleanup = () => {
      window.removeEventListener("message", handler);
      window.clearInterval(watchdog);
    };

    const handler = (event: MessageEvent) => {
      if (event.origin !== window.location.origin) return;
      const data = event.data as CallbackMessage | undefined;
      if (
        !data ||
        (data.type !== "github-callback-success" && data.type !== "github-callback-error")
      ) {
        return;
      }
      settled = true;
      cleanup();
      if (data.type === "github-callback-error") {
        reject(new GitHubCallbackError(data.reason, data.code));
        return;
      }
      if (data.flow !== expectedFlow) {
        reject(new Error(`Unexpected flow: ${data.flow}`));
        return;
      }
      resolve(data as unknown as Extract<GitHubCallbackResponse, { flow: F }>);
    };

    const watchdog = window.setInterval(() => {
      if (settled) return;
      if (popup.closed) {
        cleanup();
        reject(new GitHubCallbackCancelled());
      }
    }, POPUP_CLOSED_POLL_MS);

    window.addEventListener("message", handler);
  });
}
