/**
 * Error response from the API
 */
export interface ApiError {
  message: string;
  status: number;
  details?: unknown;
}

/**
 * PostMessage authentication protocol types
 */

/**
 * Request message sent from iframe to parent window
 */
export interface OxyAuthRequestMessage {
  type: "OXY_AUTH_REQUEST";
  version: "1.0";
  timestamp: number;
  requestId: string;
}

/**
 * Response message sent from parent window to iframe
 */
export interface OxyAuthResponseMessage {
  type: "OXY_AUTH_RESPONSE";
  version: "1.0";
  requestId: string;
  apiKey?: string;
  projectId?: string;
  baseUrl?: string;
}

/**
 * Options for postMessage authentication
 */
export interface PostMessageAuthOptions {
  /** Required parent window origin for security (e.g., 'https://app.example.com'). Use '*' only in development! */
  parentOrigin?: string;
  /** Timeout in milliseconds (default: 5000) */
  timeout?: number;
  /** Number of retry attempts (default: 0) */
  retries?: number;
}

/**
 * Result from successful postMessage authentication
 */
export interface PostMessageAuthResult {
  apiKey?: string;
  projectId?: string;
  baseUrl?: string;
  source: "postmessage";
}

/**
 * Custom error classes for postMessage authentication
 */

/**
 * Error thrown when postMessage authentication times out
 */
export class PostMessageAuthTimeoutError extends Error {
  constructor(timeout: number) {
    super(
      `Parent window did not respond to authentication request within ${timeout}ms.\n\n` +
        `Possible causes:\n` +
        `- Parent window is not listening for 'OXY_AUTH_REQUEST' messages\n` +
        `- Parent origin mismatch\n` +
        `- Network/browser issues\n\n` +
        `Troubleshooting:\n` +
        `1. Verify parent window has message listener set up\n` +
        `2. Check parentOrigin configuration matches parent window origin\n` +
        `3. Open browser console in parent window for errors`
    );
    this.name = "PostMessageAuthTimeoutError";
  }
}

/**
 * Error thrown when authentication response comes from unauthorized origin
 */
export class PostMessageAuthInvalidOriginError extends Error {
  constructor(expected: string, actual: string) {
    super(
      `Received authentication response from unauthorized origin.\n\n` +
        `Expected: ${expected}\n` +
        `Actual: ${actual}\n\n` +
        `This is a security error. Verify your parentOrigin configuration.`
    );
    this.name = "PostMessageAuthInvalidOriginError";
  }
}

/**
 * Error thrown when postMessage authentication is attempted outside iframe context
 */
export class PostMessageAuthNotInIframeError extends Error {
  constructor() {
    const currentContext = getCurrentContext();
    super(
      `PostMessage authentication is only available when running in an iframe context.\n\n` +
        `Current context: ${currentContext}\n\n` +
        `If you're running in a regular browser window, use direct configuration instead:\n` +
        `const client = new OxyClient({ apiKey: 'your-key', ... })`
    );
    this.name = "PostMessageAuthNotInIframeError";
  }
}

function getCurrentContext(): string {
  if (typeof window === "undefined") {
    return "non-browser (Node.js)";
  }
  if (window === window.parent) {
    return "top-level window";
  }
  return "iframe";
}

/**
 * Error thrown when authentication response is malformed or invalid
 */
export class PostMessageAuthInvalidResponseError extends Error {
  constructor(reason: string) {
    super(`Invalid authentication response from parent: ${reason}`);
    this.name = "PostMessageAuthInvalidResponseError";
  }
}
