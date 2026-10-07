import { AxiosError, type AxiosResponse } from "axios";
import { describe, expect, it } from "vitest";
import { sendFailureMessage } from "./sendFailureMessage";

/** The error axios rejects with when the server answers `status` with `body`. */
const answered = (status: number, body: unknown) =>
  new AxiosError(`Request failed with status code ${status}`, "ERR_BAD_REQUEST", undefined, null, {
    status,
    data: body
  } as AxiosResponse);

describe("sendFailureMessage", () => {
  it("uses the server's own sentence when it sent one", () => {
    expect(
      sendFailureMessage(
        answered(404, { code: "no_report", message: "No usage report has been written yet." })
      )
    ).toBe("No usage report has been written yet.");
  });

  it("says no report exists for a 404 that carries only a code", () => {
    // `message` is optional in the error body. Without this the toast would read
    // "Request failed with status code 404".
    expect(sendFailureMessage(answered(404, { code: "no_report" }))).toBe(
      "No report has been written yet."
    );
  });

  it("says there is no sender for a 503 that carries only a code", () => {
    expect(sendFailureMessage(answered(503, { code: "email_not_configured" }))).toBe(
      "This deployment has no email sender configured."
    );
  });

  it("never surfaces axios's status-code string", () => {
    expect(sendFailureMessage(answered(500, undefined))).toBe("Couldn't send the report.");
    expect(sendFailureMessage(answered(500, "<html>Bad gateway</html>"))).toBe(
      "Couldn't send the report."
    );
  });
});
