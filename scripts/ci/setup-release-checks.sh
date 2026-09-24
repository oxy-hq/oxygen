#!/usr/bin/env bash
# Set the secrets the custom-app release checks need, in one pass.
#
# There are three, because everything else falls back to a token the repository
# already holds (`custom-app-checks.yaml`'s header lists the chain):
#
#   OXY_API_KEY   on custom-app-checks-staging   an API key for the canary user
#   OXY_API_KEY   on custom-app-checks-prod      on that deployment
#   CUSTOM_APP_CHECKS_CHECKIN_URL  on the repo   the All Quiet check-in URL
#                                                (`terraform output`), prod only
#
# The fourth value, CHECKS_DISPATCH_TOKEN, lives on the oxy-hq/oxygen mirror and
# is set by whoever administers it; this script reports whether it is there and
# does not try to write it.
#
# Nothing is echoed, nothing is logged, and an existing secret is never
# overwritten without --force: re-running this after a partial setup is safe,
# and so is running it to see where things stand (--check).
#
# Usage:
#   scripts/ci/setup-release-checks.sh --check
#   scripts/ci/setup-release-checks.sh            # prompts for what is missing
#   scripts/ci/setup-release-checks.sh --force    # re-prompts for all of them
#   STAGING_KEY=… PROD_KEY=… CHECKIN_URL=… scripts/ci/setup-release-checks.sh
#
# `gh` must be authenticated with admin rights on oxy-hq/oxygen-internal.

set -euo pipefail

REPO="${REPO:-oxy-hq/oxygen-internal}"
MIRROR="${MIRROR:-oxy-hq/oxygen}"
STAGING_ENV="custom-app-checks-staging"
PROD_ENV="custom-app-checks-prod"
CHECKIN_SECRET="CUSTOM_APP_CHECKS_CHECKIN_URL"

check_only=false
force=false
for arg in "$@"; do
  case "$arg" in
    --check) check_only=true ;;
    --force) force=true ;;
    -h | --help)
      sed -n '2,26p' "$0" | sed 's/^# \{0,1\}//'
      exit 0
      ;;
    *)
      echo "unknown argument: $arg (try --help)" >&2
      exit 2
      ;;
  esac
done

if ! command -v gh > /dev/null; then
  echo "gh is not installed — see https://cli.github.com" >&2
  exit 1
fi
if ! gh auth status > /dev/null 2>&1; then
  echo "gh is not authenticated: run \`gh auth login\` first" >&2
  exit 1
fi

# --- what is set now -------------------------------------------------------

env_has() { # env_has <environment> <name>
  gh api "repos/${REPO}/environments/$1/secrets" --jq '.secrets[].name' 2> /dev/null |
    grep -qx "$2"
}
repo_has() { # repo_has <name>
  gh secret list --repo "${REPO}" --json name --jq '.[].name' 2> /dev/null | grep -qx "$1"
}
mirror_has() { # mirror_has <name>
  gh secret list --repo "${MIRROR}" --json name --jq '.[].name' 2> /dev/null | grep -qx "$1"
}
mark() { if "$1"; then echo "set"; else echo "MISSING"; fi; }

report() {
  echo "Release checks — what each scope holds"
  echo
  printf '  %-46s %s\n' "${STAGING_ENV}/OXY_API_KEY" "$(mark "$(
    env_has "${STAGING_ENV}" OXY_API_KEY && echo true || echo false
  )")"
  printf '  %-46s %s\n' "${PROD_ENV}/OXY_API_KEY" "$(mark "$(
    env_has "${PROD_ENV}" OXY_API_KEY && echo true || echo false
  )")"
  printf '  %-46s %s\n' "repo/${CHECKIN_SECRET}" "$(mark "$(
    repo_has "${CHECKIN_SECRET}" && echo true || echo false
  )")"
  echo
  echo "  Fallbacks in play (set the per-environment secret to override):"
  printf '    %-44s %s\n' "INFRA_PR_TOKEN → repo/OXY_HQ_GIT_TOKEN" "$(mark "$(
    repo_has OXY_HQ_GIT_TOKEN && echo true || echo false
  )")"
  printf '    %-44s %s\n' "SENTRY_*_TOKEN → repo/SENTRY_AUTH_TOKEN" "$(mark "$(
    repo_has SENTRY_AUTH_TOKEN && echo true || echo false
  )")"
  echo
  printf '  %-46s %s\n' "${MIRROR}/CHECKS_DISPATCH_TOKEN" "$(mark "$(
    mirror_has CHECKS_DISPATCH_TOKEN && echo true || echo false
  )")"
  echo "    (set by whoever administers the mirror; this script does not write it)"
}

if "${check_only}"; then
  report
  exit 0
fi

# --- set what is missing ---------------------------------------------------

# read_secret <prompt-var> <fallback-prompt> — from the environment if given,
# else prompted without echo. Never printed, never stored on disk.
read_secret() {
  local var="$1" prompt="$2" value="${!1:-}"
  if [[ -z "${value}" ]]; then
    read -rsp "${prompt}: " value < /dev/tty
    echo >&2
  fi
  printf '%s' "${value}"
}

set_env_secret() { # set_env_secret <environment> <name> <prompt> <var>
  local environment="$1" name="$2" prompt="$3" var="$4"
  if env_has "${environment}" "${name}" && ! "${force}"; then
    echo "  ${environment}/${name} is already set — leaving it (use --force to replace)"
    return
  fi
  local value
  value="$(read_secret "${var}" "${prompt}")"
  if [[ -z "${value}" ]]; then
    echo "  ${environment}/${name}: nothing entered, skipped"
    return
  fi
  printf '%s' "${value}" | gh secret set "${name}" --repo "${REPO}" --env "${environment}" --body -
  echo "  ${environment}/${name} set"
}

echo "Setting what is missing. Nothing is echoed; an existing secret is kept unless --force."
echo
set_env_secret "${STAGING_ENV}" OXY_API_KEY "staging canary API key" STAGING_KEY
set_env_secret "${PROD_ENV}" OXY_API_KEY "prod canary API key" PROD_KEY

if repo_has "${CHECKIN_SECRET}" && ! "${force}"; then
  echo "  repo/${CHECKIN_SECRET} is already set — leaving it (use --force to replace)"
else
  url="$(read_secret CHECKIN_URL "All Quiet check-in URL for prod (terraform output)")"
  if [[ -z "${url}" ]]; then
    echo "  repo/${CHECKIN_SECRET}: nothing entered, skipped"
  else
    printf '%s' "${url}" | gh secret set "${CHECKIN_SECRET}" --repo "${REPO}" --body -
    echo "  repo/${CHECKIN_SECRET} set"
  fi
fi

echo
report
echo
echo "Next: publish the canary on an environment already running the release that"
echo "carries its refusal steps, then dispatch once by hand:"
echo "  gh workflow run custom-app-checks.yaml --repo ${REPO} -f environment=staging"
echo "The schedule needs no switch — a leg with a key runs, one without exits green."
