// Post files into a Slack thread with the external-upload flow:
// getUploadURLExternal per file, the bytes to the URL it returns, then one
// completeUploadExternal that shares them all in the thread with a comment.
// Needs a bot token with `files:write`, and the bot in the channel.
//
// Slack answers 200 with {"ok": false} for most failures, so every response
// body is read — a status code proves nothing (.github/scripts/slack-post.sh).

import { readFileSync, statSync } from "node:fs";
import { basename } from "node:path";

export interface UploadRequest {
  token: string;
  /** Channel ID (C…), as chat.postMessage returns it — not the #name. */
  channel: string;
  threadTs: string;
  comment: string;
  files: { path: string; title: string }[];
}

type Fetch = typeof fetch;

async function slackCall(
  f: Fetch,
  token: string,
  method: string,
  params: Record<string, string>
): Promise<Record<string, unknown>> {
  const res = await f(`https://slack.com/api/${method}`, {
    method: "POST",
    headers: {
      Authorization: `Bearer ${token}`,
      "Content-Type": "application/x-www-form-urlencoded"
    },
    body: new URLSearchParams(params)
  });
  const body = (await res.json()) as Record<string, unknown>;
  if (body.ok !== true) throw new Error(`Slack ${method}: ${String(body.error ?? res.status)}`);
  return body;
}

export async function uploadToThread(req: UploadRequest, f: Fetch = fetch): Promise<void> {
  const uploaded: { id: string; title: string }[] = [];
  for (const file of req.files) {
    const { upload_url, file_id } = (await slackCall(f, req.token, "files.getUploadURLExternal", {
      filename: basename(file.path),
      length: String(statSync(file.path).size)
    })) as { upload_url: string; file_id: string };
    const put = await f(upload_url, { method: "POST", body: readFileSync(file.path) });
    if (!put.ok) throw new Error(`Slack upload of ${basename(file.path)} answered ${put.status}`);
    uploaded.push({ id: file_id, title: file.title });
  }
  await slackCall(f, req.token, "files.completeUploadExternal", {
    files: JSON.stringify(uploaded),
    channel_id: req.channel,
    thread_ts: req.threadTs,
    initial_comment: req.comment
  });
}
