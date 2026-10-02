// SSE reader for `/fn/<name>` responses (design doc §11.2).
//
// The route emits zero or more `event: log` frames (the function's
// `console.*` / `ctx.log` output — collected during the run and sent with the
// response, not live-tailed — so a developer doesn't have to open the oxy server
// logs), then terminates with either an `event: done` frame (whose accumulated
// `data` carries the JSON-encoded function result) or an `event: error` frame
// (structured `{ error, message }`).
// Extracted from the React hook so it can be unit-tested without a DOM/render.

/** A captured `console.*` / `ctx.log` line from a function run. */
export interface FunctionLog {
  level: string;
  message: string;
}

/** A successful function result plus the logs captured during the run. */
export interface FunctionResult<Data> {
  value: Data;
  logs: FunctionLog[];
}

/**
 * An error carries the logs captured before the throw, so the app can show them.
 *
 * `status` is the HTTP status the FUNCTION returned, present when the function
 * ran and answered a non-2xx. It is absent when the run itself failed (an
 * `event: error` frame — a crash, a timeout, a cancellation), because there was
 * no response to have a status.
 *
 * `body` is the parsed payload the function returned with that status, so a
 * caller can read `{ error: "…" }` without re-parsing the message.
 */
export type FunctionError = Error & {
  logs?: FunctionLog[];
  status?: number;
  body?: unknown;
  /**
   * The platform trace this invoke ran in (32 hex chars) and the server-minted
   * `x-oxy-request-id`, when the request got as far as the server. Quote
   * either in a bug report: an operator can open the trace in HyperDX, and
   * an app admin can filter the app's Logs by the request.
   */
  traceId?: string;
  requestId?: string;
};

/**
 * Read a `text/event-stream` function response to completion. Resolves with the
 * decoded result + captured logs, or rejects (with `.logs` attached) on an
 * `event: error` frame / a stream that ends without a terminal event.
 */
export async function readFunctionSseStream<Data>(resp: Response): Promise<FunctionResult<Data>> {
  const reader = resp.body?.getReader();
  if (!reader) {
    throw new Error("function response has no body stream");
  }
  const decoder = new TextDecoder();
  let buffer = "";
  let dataPayload = "";
  const logs: FunctionLog[] = [];

  const handleFrame = (frame: string): { done: true; value: Data } | undefined => {
    let event = "message";
    let data = "";
    for (const line of frame.split("\n")) {
      if (line.startsWith("event:")) event = line.slice(6).trim();
      else if (line.startsWith("data:")) data += line.slice(5).trim();
    }
    if (event === "log") {
      try {
        const l = JSON.parse(data);
        logs.push({ level: String(l.level ?? "info"), message: String(l.message ?? "") });
      } catch {
        // Ignore a malformed log frame rather than fail the whole invocation.
      }
    } else if (event === "data") {
      dataPayload = data;
    } else if (event === "done") {
      const parsed = dataPayload ? (JSON.parse(dataPayload) as unknown) : null;
      // The status the function returned. The route used to hardcode 200 here,
      // so a handler answering 403 or 409 resolved as an ordinary success and
      // every caller had to infer rejection from the body's shape — which meant
      // a `catch` written for it was dead code that never ran.
      //
      // Absent or non-numeric is treated as success: an older server does not
      // send one, and an app talking to it must keep working rather than start
      // throwing on every call.
      const meta = data ? (JSON.parse(data) as { status?: unknown }) : {};
      const status = typeof meta.status === "number" ? meta.status : 200;
      if (status < 200 || status >= 300) {
        const payload = parsed as { error?: unknown; message?: unknown } | null;
        // A function may answer with a structured `error`; as a message that
        // has to be its JSON, not "[object Object]".
        const text = (v: unknown) =>
          typeof v === "string" ? v : v === null || v === undefined ? undefined : JSON.stringify(v);
        const err = new Error(
          text(payload?.message) ?? text(payload?.error) ?? `function returned ${status}`
        ) as FunctionError;
        err.name = "FunctionStatusError";
        err.status = status;
        err.body = parsed;
        err.logs = logs;
        throw err;
      }
      return { done: true, value: parsed as Data };
    } else if (event === "error") {
      const payload = data ? JSON.parse(data) : {};
      const err = new Error(
        payload.message || payload.error || "function invocation failed"
      ) as FunctionError;
      err.name = payload.error || "FunctionError";
      err.logs = logs;
      throw err;
    }
    return undefined;
  };

  for (;;) {
    const { done, value } = await reader.read();
    if (done) break;
    buffer += decoder.decode(value, { stream: true });
    let sep: number;
    while ((sep = buffer.indexOf("\n\n")) !== -1) {
      const frame = buffer.slice(0, sep);
      buffer = buffer.slice(sep + 2);
      const result = handleFrame(frame);
      if (result) return { value: result.value, logs };
    }
  }
  throw new Error("function stream ended without a terminal event");
}
