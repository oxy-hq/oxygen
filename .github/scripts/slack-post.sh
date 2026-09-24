#!/usr/bin/env bash
# Post one message to the deploy channel, or say why it could not.
#
# Shared by every notification in promote.yaml so the "token is not set" and
# "Slack said no" paths are written once. chat.postMessage answers 200 with
# {"ok": false} on a bad channel or a revoked token, so curl's exit status
# proves nothing — the body has to be read. Same shape as
# oxy-hq/infrastructure's oxy-prod-announce.yaml, deliberately.
#
# A missing SLACK_BOT_TOKEN is a warning and a printed message, not a failure:
# the notification is how a human hears, and losing it must not also stop the
# promotion it was describing.
#
# Usage: SLACK_BOT_TOKEN=… slack-post.sh "<message>"
set -euo pipefail

TEXT="${1:?usage: slack-post.sh <message>}"
CHANNEL="${SLACK_CHANNEL:-product-releases-prod}"

if [[ -z "${SLACK_BOT_TOKEN:-}" ]]; then
  echo "::warning::SLACK_BOT_TOKEN is not set — printing the message instead of posting."
  printf '%s\n' "$TEXT"
  exit 0
fi

# `RUNNER_TEMP` on a runner; anything else locally. `mktemp`'s default lands in
# a directory a sandboxed shell may not write to, and this script is also run by
# hand to check the wiring.
PAYLOAD="${RUNNER_TEMP:-${TMPDIR:-/tmp}}/slack-post.$$.json"
jq -n --arg channel "$CHANNEL" --arg text "$TEXT" \
  '{channel: $channel, text: $text, unfurl_links: false, unfurl_media: false}' > "$PAYLOAD"

RESP=$(curl -sS -X POST https://slack.com/api/chat.postMessage \
  -H "Authorization: Bearer ${SLACK_BOT_TOKEN}" \
  -H 'Content-type: application/json; charset=utf-8' \
  --data @"$PAYLOAD")

if [[ "$(jq -r '.ok' <<<"$RESP")" != "true" ]]; then
  # Loud, but not fatal — the header's rule, applied to the case it forgot. A
  # revoked token exiting 1 fails the step under `set -e`, which drops the marker
  # comment written after this call; without the marker the notification is due
  # again next pass, so a broken token would also mean a repeating one.
  echo "::error::Slack rejected the message: $(jq -r '.error // "unknown"' <<<"$RESP")"
  printf '%s\n' "$TEXT"
  exit 0
fi
echo "Posted to #${CHANNEL}."
