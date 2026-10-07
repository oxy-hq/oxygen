import { apiErrorMessage, apiStatus } from "@/libs/apiError";

/**
 * What to say when "Send me the latest report" is refused.
 *
 * The server's own sentence when it sent one. Its error body is `{ code, message? }`, so
 * the message can be missing, and axios's stand-in ("Request failed with status code
 * 404") tells a reader nothing — the two refusals this route is known to make each get a
 * plain sentence of their own instead.
 */
export function sendFailureMessage(err: unknown): string {
  const status = apiStatus(err);
  const fallback =
    status === 404
      ? "No report has been written yet."
      : status === 503
        ? "This deployment has no email sender configured."
        : "Couldn't send the report.";
  return apiErrorMessage(err, fallback);
}
