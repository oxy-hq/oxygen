/**
 * PostMessage-based authentication for iframe scenarios
 *
 * This module enables secure cross-origin authentication between an iframe
 * (running the Oxy SDK) and its parent window (holding the API key).
 */

import type {
  OxyAuthRequestMessage,
  OxyAuthResponseMessage,
  PostMessageAuthOptions,
  PostMessageAuthResult
} from "../types";

import {
  PostMessageAuthInvalidResponseError,
  PostMessageAuthNotInIframeError,
  PostMessageAuthTimeoutError
} from "../types";

/**
 * Check if the current context is running inside an iframe
 *
 * @returns true if running in an iframe, false otherwise (including Node.js)
 */
export function isInIframe(): boolean {
  // Check if we're in a browser environment
  if (typeof window === "undefined") {
    return false;
  }

  // Check if window has a parent and it's different from itself
  try {
    return window !== window.parent && window.parent !== null;
  } catch {
    // Cross-origin access might throw an error, but we're still in an iframe
    return true;
  }
}

/**
 * Generate a unique request ID for tracking auth requests
 *
 * @returns A unique identifier string
 */
export function generateRequestId(): string {
  // Use crypto.randomUUID if available (modern browsers)
  if (typeof crypto !== "undefined" && crypto.randomUUID) {
    return crypto.randomUUID();
  }

  // Fallback for older environments: timestamp + random
  // Note: Math.random() is used here as a fallback for environments without crypto.randomUUID
  // This is acceptable for non-security-critical request IDs (used only for message correlation)
  /* eslint-disable sonarjs/pseudo-random */
  return `${Date.now()}-${Math.random().toString(36).substring(2, 11)}`;
  /* eslint-enable sonarjs/pseudo-random */
}

/**
 * Validate that a message origin matches the expected origin
 *
 * @param messageOrigin - The origin from the message event
 * @param allowedOrigin - The expected/allowed origin
 * @returns true if origin is valid, false otherwise
 */
export function validateOrigin(messageOrigin: string, allowedOrigin?: string): boolean {
  // If no allowed origin is specified, reject (security)
  if (!allowedOrigin) {
    return false;
  }

  // Wildcard - allow any origin (development only!)
  if (allowedOrigin === "*") {
    return true;
  }

  // Exact match
  if (messageOrigin === allowedOrigin) {
    return true;
  }

  // Try URL parsing for more flexible matching
  try {
    const messageUrl = new URL(messageOrigin);
    const allowedUrl = new URL(allowedOrigin);
    return messageUrl.origin === allowedUrl.origin;
  } catch {
    // If URL parsing fails, do simple string match
    return messageOrigin === allowedOrigin;
  }
}

/**
 * Create a promise that listens for an authentication response
 *
 * @param requestId - The request ID to match against
 * @param origin - The expected parent origin
 * @param timeout - Timeout in milliseconds
 * @returns Promise that resolves with the auth response
 */
function createAuthListener(
  requestId: string,
  origin: string | undefined,
  timeout: number
): Promise<OxyAuthResponseMessage> {
  return new Promise((resolve, reject) => {
    const listener = (event: MessageEvent) => {
      if (!validateOrigin(event.origin, origin)) {
        // Don't reject here - might be other postMessage traffic
        // Just ignore messages from wrong origins
        return;
      }

      if (event.data?.type !== "OXY_AUTH_RESPONSE") {
        // Not our message type, ignore
        return;
      }

      const response = event.data as OxyAuthResponseMessage;

      // Validate request ID matches
      if (response.requestId !== requestId) {
        // Wrong request ID, ignore (might be another auth in progress)
        return;
      }

      // Validate version
      if (response.version !== "1.0") {
        clearTimeout(timeoutId);
        window.removeEventListener("message", listener);
        reject(
          new PostMessageAuthInvalidResponseError(
            // The type says "1.0"; this branch is what arrives when it is not.
            `Unsupported protocol version: ${JSON.stringify(response.version)}`
          )
        );
        return;
      }

      // Success!
      clearTimeout(timeoutId);
      window.removeEventListener("message", listener);
      resolve(response);
    };

    const timeoutId = setTimeout(() => {
      window.removeEventListener("message", listener);
      reject(new PostMessageAuthTimeoutError(timeout));
    }, timeout);

    // Start listening
    window.addEventListener("message", listener);
  });
}

/**
 * Request authentication from parent window via postMessage
 *
 * This is the main entry point for iframe-based authentication.
 * It sends a request to the parent window and waits for a response.
 *
 * @param options - Configuration options for the auth request
 * @returns Promise that resolves with authentication credentials
 * @throws {PostMessageAuthNotInIframeError} If not in an iframe
 * @throws {PostMessageAuthTimeoutError} If parent doesn't respond in time
 * @throws {PostMessageAuthInvalidOriginError} If response from wrong origin
 * @throws {PostMessageAuthInvalidResponseError} If response is malformed
 *
 * @example
 * ```typescript
 * const auth = await requestAuthFromParent({
 *   parentOrigin: 'https://app.example.com',
 *   timeout: 5000
 * });
 * console.log('Received API key:', auth.apiKey);
 * ```
 */
export async function requestAuthFromParent(
  options: PostMessageAuthOptions = {}
): Promise<PostMessageAuthResult> {
  const { parentOrigin, timeout = 5000, retries = 0 } = options;

  if (!isInIframe()) {
    throw new PostMessageAuthNotInIframeError();
  }

  // Validate we're in a browser environment
  if (typeof window === "undefined") {
    throw new PostMessageAuthNotInIframeError();
  }

  const requestId = generateRequestId();

  const request: OxyAuthRequestMessage = {
    type: "OXY_AUTH_REQUEST",
    version: "1.0",
    timestamp: Date.now(),
    requestId
  };

  let lastError: Error | null = null;
  const maxAttempts = retries + 1;

  for (let attempt = 0; attempt < maxAttempts; attempt++) {
    try {
      // Set up listener before sending request to avoid race condition
      const responsePromise = createAuthListener(requestId, parentOrigin, timeout);

      const targetOrigin = parentOrigin || "*";
      window.parent.postMessage(request, targetOrigin);

      // Wait for response
      const response = await responsePromise;

      // Return successful result
      return {
        apiKey: response.apiKey,
        projectId: response.projectId,
        baseUrl: response.baseUrl,
        source: "postmessage"
      };
    } catch (error) {
      lastError = error as Error;

      // If this isn't a timeout, don't retry
      if (!(error instanceof PostMessageAuthTimeoutError)) {
        throw error;
      }

      if (attempt < maxAttempts - 1) {
        // Optional: Add a small delay before retry
        await new Promise((resolve) => setTimeout(resolve, 100));
        continue;
      }

      // All retries exhausted
      throw error;
    }
  }

  // Should never reach here, but TypeScript needs this
  throw lastError || new Error("Authentication failed");
}
