#!/usr/bin/env bash
# SessionStart hook for Claude Code cloud sessions (claude.ai/code): makes
# `oxyc` usable with no browser on this machine — installed, routed through the
# session's egress proxy, and given a credential that survives between Bash
# calls. A local session exits at once.
#
# stdout is read by Claude, so it says only what is missing and the one thing
# to do about it. The cloud *environment* still needs the Oxy hosts allowed
# (Network access → Custom): the oxy-custom-apps skill, "Headless and cloud
# sessions".
#
# Two copies, kept identical by a test: oxygen-internal's own, and the one in
# sdk/cli/template/ that `oxyc update` writes into every customer repo (it is
# managed there — edit it in oxygen-internal, not in the customer repo).

[ "${CLAUDE_CODE_REMOTE:-}" = "true" ] || exit 0
cat >/dev/null 2>&1 || true

# Where a minted agent token is kept: the one `export OXY_TOKEN=…` line that
# `oxyc tokens create --agent` prints on stdout.
TOKEN_FILE="$HOME/.config/oxy/agent-token.env"
ENV="${OXY_ENV:-production}"
case "$ENV" in
  production) HOST=app.oxygen-hq.com ;;
  staging) HOST=aip.staging.oxy.tech ;;
  dev) HOST=aip.dev.oxy.tech ;;
  *) HOST="" ;;
esac

# Installed, and current. A cloud image can ship an oxyc older than the
# commands the docs and skills name (one shipped 0.11 when they named 0.12's
# `oxyc login --no-wait`), so an older one is upgraded, not only a missing one.
# A version this cannot read is left alone rather than replaced.
have=$(oxyc --version 2>/dev/null | tail -n 1)
if ! command -v oxyc >/dev/null 2>&1; then
  npm install -g @oxy-hq/cli >/dev/null 2>&1 \
    || echo "oxyc: 'npm install -g @oxy-hq/cli' failed — run it before any oxyc command."
elif [[ $have =~ ^[0-9]+\.[0-9]+\.[0-9]+$ ]]; then
  # A registry that does not answer costs 20 s at most, not the hook's budget.
  limit=()
  command -v timeout >/dev/null 2>&1 && limit=(timeout 20)
  want=$("${limit[@]}" npm view @oxy-hq/cli version 2>/dev/null | tail -n 1)
  if [[ $want =~ ^[0-9]+\.[0-9]+\.[0-9]+$ ]] && [ "$want" != "$have" ] &&
    [ "$(printf '%s\n%s\n' "$have" "$want" | sort -V | tail -n 1)" = "$want" ]; then
    npm install -g "@oxy-hq/cli@$want" >/dev/null 2>&1 \
      || echo "oxyc: $have is installed and 'npm install -g @oxy-hq/cli@$want' failed — commands newer than $have will not exist."
  fi
fi

if [ -n "${CLAUDE_ENV_FILE:-}" ]; then
  {
    # Node's fetch ignores HTTPS_PROXY without this, and every request out of
    # this VM goes through one. A current oxyc sets it itself; older ones don't.
    echo 'export NODE_USE_ENV_PROXY=1'
    # Runs before every Bash command, so a token minted mid-session reaches
    # every command after it — an `export` in one call would not.
    echo "[ -f \"$TOKEN_FILE\" ] && . \"$TOKEN_FILE\""
  } >>"$CLAUDE_ENV_FILE"
fi

if [ -n "$HOST" ] && ! curl -fsS --max-time 10 -o /dev/null "https://$HOST/api/health" 2>/dev/null; then
  echo "oxyc: https://$HOST is unreachable from this session. The cloud environment's network"
  echo "  access must allow it (Custom: app.oxygen-hq.com, *.oxy.tech) — ask the user to add it."
fi

if [ "${OXY_TOKEN:-}" = "from-proxy" ]; then
  # The environment's network secret: its proxy adds Authorization on the way
  # out, so no token is in this VM. A minted token file, sourced before every
  # command, replaces the sentinel.
  if [ -s "$TOKEN_FILE" ]; then
    echo "oxyc: OXY_TOKEN=from-proxy, but the agent token in $TOKEN_FILE replaces it in every command."
  elif command -v oxyc >/dev/null 2>&1 && ! oxyc whoami --env "$ENV" >/dev/null 2>&1; then
    echo "oxyc: OXY_TOKEN=from-proxy (a network secret), but 'oxyc whoami --env $ENV' fails. Run it to"
    echo "  see why, and tell the user: the secret's Allowed websites must cover ${HOST:-the Oxy host}, with"
    echo "  header Authorization and prefix Bearer. Do not mint a token in its place without asking."
  else
    echo "oxyc: credential is the environment's network secret (OXY_TOKEN=from-proxy); no token is in this VM."
  fi
elif [ -n "${OXY_TOKEN:-}" ]; then
  echo "oxyc: credential is OXY_TOKEN from the environment."
elif [ -s "$TOKEN_FILE" ]; then
  echo "oxyc: credential is the agent token in $TOKEN_FILE, good on the one deployment that minted it"
  echo "  (it expires: on exit 4, mint again)."
else
  cat <<EOF
oxyc: no Oxy credential yet. A token is good on one deployment (production, staging or dev), so
  mint an agent token for the one the task uses. Start it with the Bash tool's run_in_background:
    mkdir -p ~/.config/oxy && (umask 077; oxyc tokens create --agent --no-browser --env $ENV >$TOKEN_FILE)
  It ends only once a person approves, so do not wait for it: a few seconds in, Read the task's
  output file for the approval URL and code (the token goes to the file, never to you) and give
  both to the user. --approver <their email> (or OXY_APPROVER) also pushes the request to them.
  Once approved, every later command has OXY_TOKEN. It reaches what the approver's org
  memberships do, never staff access: no device-code approval grants that. To sign a browser in,
  navigate to the one-time URL 'oxyc login-link --env $ENV --next <path>' prints, never to the
  Google, GitHub or email sign-in. See the oxy-custom-apps skill, "Headless and cloud sessions".
EOF
fi
exit 0
