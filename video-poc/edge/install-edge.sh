#!/usr/bin/env bash
# Oxy edge box installer.
#
# Provisions a self-purchased COTS box (Jetson Orin Nano,
# Intel N100, generic x86 server, etc.) into a workspace's
# fleet in one command. Designed to be invoked from the
# "Add device" wizard's copy-paste snippet:
#
#   curl -sSL https://<your-oxy>/install-edge.sh \
#     | sudo bash -s -- \
#         --device-id <uuid> \
#         --device-secret <base64> \
#         --oxy-url https://<your-oxy>
#
# Or driven from a checked-out repo locally:
#
#   sudo ./install-edge.sh \
#       --device-id <uuid> \
#       --device-secret <base64> \
#       --oxy-url https://<your-oxy> \
#       --hardware-label "Jetson Orin Nano"
#
# What it does, in order:
#   1. Verify dependencies (curl, docker, docker compose v2).
#      Installs docker via the convenience script if missing.
#   2. Create /var/lib/oxy and write the device identity JSON
#      at 0600 perms.
#   3. Create /opt/oxy-edge, drop the docker-compose.yml and
#      the .env (with OXY_URL) into it.
#   4. Install /etc/systemd/system/oxy-edge.service and
#      `systemctl enable --now` it.
#   5. Tail the unit's journal for ~10s so the operator sees
#      the boot succeed without separately running journalctl.
#
# Idempotency: re-running with the same args overwrites
# device.json + .env + the unit file but leaves running
# containers alone (systemctl restart picks up the new
# compose). Re-running with DIFFERENT device-id rotates the
# identity — which is a destructive operation; the script
# refuses unless --force is passed.
#
# Exits non-zero on any step failure; safe for `set -e`
# parent contexts.

set -euo pipefail

# ── Defaults ─────────────────────────────────────────────────

INSTALL_ROOT="/opt/oxy-edge"
IDENTITY_DIR="/var/lib/oxy"
SERVICE_NAME="oxy-edge.service"
SYSTEMD_PATH="/etc/systemd/system/${SERVICE_NAME}"

# These two come from the same repo the install script does;
# the wizard's copy-paste hard-codes the right tag for the
# build the operator is on.
COMPOSE_URL_DEFAULT="https://raw.githubusercontent.com/oxy-hq/oxy/main/video-poc/docker-compose.yml"
SYSTEMD_URL_DEFAULT="https://raw.githubusercontent.com/oxy-hq/oxy/main/video-poc/edge/oxy-edge.service"
ENABLE_SUPPORT_URL_DEFAULT="https://raw.githubusercontent.com/oxy-hq/oxy/main/video-poc/edge/enable-remote-support.sh"
DISABLE_SUPPORT_URL_DEFAULT="https://raw.githubusercontent.com/oxy-hq/oxy/main/video-poc/edge/disable-remote-support.sh"

DEVICE_ID=""
DEVICE_SECRET=""
OXY_URL=""
HARDWARE_LABEL="unspecified"
SKU="self-installed"
COMPOSE_URL="${OXY_COMPOSE_URL:-${COMPOSE_URL_DEFAULT}}"
SYSTEMD_URL="${OXY_SYSTEMD_URL:-${SYSTEMD_URL_DEFAULT}}"
ENABLE_SUPPORT_URL="${OXY_ENABLE_SUPPORT_URL:-${ENABLE_SUPPORT_URL_DEFAULT}}"
DISABLE_SUPPORT_URL="${OXY_DISABLE_SUPPORT_URL:-${DISABLE_SUPPORT_URL_DEFAULT}}"
FORCE=0
SKIP_DOCKER_INSTALL=0
# Tailscale auth key — short-lived (`tskey-auth-…`), tagged
# `tag:edge-box`. When set, the tailscale sidecar logs in
# automatically on first boot and the `tailscale-funnel-init`
# service publishes MTX's WHEP port via Funnel. Live preview
# then works in the operator UI without any further hand-config.
# Without it, the rest of the pipeline (events, compliance
# reports, recordings) still works — only live preview is gated.
TS_AUTHKEY=""
# Beta-support escape hatch (default off — see
# `internal-docs/video-processing-fleet-architecture.md` § Remote
# access). When --allow-remote-support is set, the installer runs
# enable-remote-support.sh after the main service is up, which
# joins the host to the tailnet as tag:edge-box-host with
# Tailscale SSH on. Requires --ts-host-authkey (a key permitted
# to carry tag:edge-box-host). Without --allow-remote-support, no
# host SSH path is created at all.
ALLOW_REMOTE_SUPPORT=0
TS_HOST_AUTHKEY=""
# Rotation hygiene: on re-run, the previous TS_AUTHKEY stays in
# .env unless we either (a) pass a fresh --ts-authkey or (b) pass
# --clear-ts-authkey. Real-world flow: an operator rotated the
# key in the Tailscale admin and wants the box to forget the old
# one without yet having a replacement.
CLEAR_TS_AUTHKEY=0
# WebRTC shared secret — must equal `OXY_CAMERAS_TURN_AUTH_SECRET` on
# the Oxy server. coturn's `--use-auth-secret` validates browser-side
# TURN credentials via HMAC against this; without it WebRTC live
# preview either rejects every session or fails to bring up coturn at
# all (compose's `${TURN_AUTH_SECRET:?...}` errors fast). The wizard's
# install snippet pre-fills this from Oxy's running env so the
# operator doesn't have to copy a value between two systems.
TURN_AUTH_SECRET=""
# HLS-only / no-WebRTC mode. When set, install writes a placeholder
# `TURN_AUTH_SECRET=disabled-no-webrtc` so compose's `${TURN_AUTH_SECRET:?...}`
# passes and coturn starts, but the HMAC won't match anything Oxy mints
# so WebRTC sessions just fail at auth — intentional. (Real
# profile-based skip is a future follow-up; placeholder is simpler
# and doesn't break the existing fleet.)
NO_WEBRTC=0
NO_WEBRTC_PLACEHOLDER="disabled-no-webrtc"
# Anthropic API key for the worker's on-trigger Haiku VLM compliance
# checks. Optional: when unset the worker logs `edge.vlm_disabled`
# and skips compliance reports; events still flow. Passed through to
# `/opt/oxy-edge/.env` so docker-compose picks it up at runtime.
ANTHROPIC_API_KEY=""

# Comma-separated camera-name substrings whose (near-field-mic) audio is run
# through the upsell-detection pipeline (worker/upsell*.py). Optional + opt-in:
# unset ⇒ audio upsell detection off. Needs ANTHROPIC_API_KEY (above). Passed
# through to `/opt/oxy-edge/.env` so docker-compose picks it up at runtime.
UPSELL_CAMERAS=""

# Length (seconds) of the congestion evidence clip. Optional; unset ⇒ the
# worker default (30s). Set to 900 for 15-min clips. Passed through to
# `/opt/oxy-edge/.env` so docker-compose picks it up at runtime.
CONGESTION_CLIP_SEC=""

# Workload role: full (cameras + audio, default) | video | audio (a dedicated
# upsell-audio-only box). Optional; empty ⇒ the compose default (full). Passed
# through to `/opt/oxy-edge/.env` so docker-compose picks it up at runtime.
EDGE_ROLE=""

# ── Output helpers ───────────────────────────────────────────

log() { printf '\033[1;34m[oxy-edge]\033[0m %s\n' "$*"; }
warn() { printf '\033[1;33m[oxy-edge]\033[0m %s\n' "$*" >&2; }
die() {
    printf '\033[1;31m[oxy-edge]\033[0m %s\n' "$*" >&2
    exit 1
}

# ── Arg parsing ──────────────────────────────────────────────

usage() {
    cat <<EOF
Usage: $0 [options]

Required:
  --device-id <uuid>           Device UUID (from the Add device wizard)
  --device-secret <base64>     Device secret, base64-encoded (no padding)
  --oxy-url <url>              Control-plane URL (e.g. https://oxy.example.com)

Optional:
  --hardware-label <text>          Free-text label; stamped on device_registry
  --ts-authkey <tskey-auth-…>      Tailscale auth key (tagged tag:edge-box).
                                    Enables live preview without further setup.
  --clear-ts-authkey               Drop any TS_AUTHKEY currently in .env (without
                                    requiring a fresh --ts-authkey). Useful after
                                    rotating the key in the Tailscale admin so
                                    the previous value doesn't sit on disk.
  --turn-auth-secret <hex>         coturn HMAC secret for WebRTC TURN credentials.
                                    Must equal OXY_CAMERAS_TURN_AUTH_SECRET on the
                                    Oxy server. The Add-device wizard pre-fills
                                    this in the install snippet automatically;
                                    pass explicitly only for standalone installs.
  --no-webrtc                      HLS-only install. Skips coturn + funnel-init
                                    TURN apply. No TURN secret required.
  --anthropic-api-key <key>        Anthropic API key for the worker's Haiku VLM
                                    compliance checks. Optional: omit to install
                                    with compliance reports disabled (events
                                    still flow).
  --upsell-cameras <list>          Comma-separated camera-name substrings whose
                                    audio runs through the upsell-detection
                                    pipeline (e.g. "PH - Santa Clara - Register").
                                    Optional; needs --anthropic-api-key. Omit to
                                    leave audio upsell detection off.
  --congestion-clip-sec <seconds>  Length of the congestion evidence clip.
                                    Optional; omit for the worker default (30).
                                    Set 900 for 15-min clips (see the compose
                                    note on clip size + annotation CPU).
  --edge-role <full|video|audio>   Workload role. Default full (cameras + audio).
                                    'audio' = a dedicated upsell-audio-only box
                                    (no YOLO / MediaMTX / preview).
  --allow-remote-support           Opt this box in to Oxy operator SSH-via-tailnet.
                                    Default OFF. When set, requires --ts-host-authkey.
                                    Can be enabled later with
                                    /opt/oxy-edge/enable-remote-support.sh.
  --ts-host-authkey <tskey-auth-…> Tailscale auth key permitted to carry
                                    tag:edge-box-host. Required with
                                    --allow-remote-support.
  --compose-url <url>              Override the docker-compose.yml source
  --systemd-url <url>              Override the systemd unit source
  --skip-docker-install            Don't install docker even if missing
  --force                          Overwrite a different existing device identity
  -h, --help                       Show this message
EOF
}

while [[ $# -gt 0 ]]; do
    case "$1" in
        --device-id) DEVICE_ID="$2"; shift 2 ;;
        --device-secret) DEVICE_SECRET="$2"; shift 2 ;;
        --oxy-url) OXY_URL="$2"; shift 2 ;;
        --hardware-label) HARDWARE_LABEL="$2"; shift 2 ;;
        --ts-authkey) TS_AUTHKEY="$2"; shift 2 ;;
        --clear-ts-authkey) CLEAR_TS_AUTHKEY=1; shift ;;
        --turn-auth-secret) TURN_AUTH_SECRET="$2"; shift 2 ;;
        --no-webrtc) NO_WEBRTC=1; shift ;;
        --anthropic-api-key) ANTHROPIC_API_KEY="$2"; shift 2 ;;
        --upsell-cameras) UPSELL_CAMERAS="$2"; shift 2 ;;
        --congestion-clip-sec) CONGESTION_CLIP_SEC="$2"; shift 2 ;;
        --edge-role) EDGE_ROLE="$2"; shift 2 ;;
        --allow-remote-support) ALLOW_REMOTE_SUPPORT=1; shift ;;
        --ts-host-authkey) TS_HOST_AUTHKEY="$2"; shift 2 ;;
        --compose-url) COMPOSE_URL="$2"; shift 2 ;;
        --systemd-url) SYSTEMD_URL="$2"; shift 2 ;;
        --skip-docker-install) SKIP_DOCKER_INSTALL=1; shift ;;
        --force) FORCE=1; shift ;;
        -h|--help) usage; exit 0 ;;
        *) warn "unknown arg: $1"; usage; exit 2 ;;
    esac
done

[[ -n "$DEVICE_ID" ]] || die "missing --device-id"
[[ -n "$DEVICE_SECRET" ]] || die "missing --device-secret"
[[ -n "$OXY_URL" ]] || die "missing --oxy-url"

if [[ "$ALLOW_REMOTE_SUPPORT" -eq 1 && -z "$TS_HOST_AUTHKEY" ]]; then
    die "--allow-remote-support requires --ts-host-authkey (a key permitted to carry tag:edge-box-host)"
fi

# WebRTC is the default — fail fast if the operator forgot to copy the
# TURN secret AND didn't opt out of WebRTC. The wizard's install snippet
# wires --turn-auth-secret automatically when the server has the env set,
# so a missing one here usually means standalone install + missed step.
if [[ "$NO_WEBRTC" -eq 0 && -z "$TURN_AUTH_SECRET" ]]; then
    die "missing --turn-auth-secret (HMAC shared with OXY_CAMERAS_TURN_AUTH_SECRET on the Oxy server). Pass --no-webrtc to install HLS-only."
fi

if [[ "$(id -u)" -ne 0 ]]; then
    die "must run as root (sudo); the script writes to /var/lib/oxy, /opt/oxy-edge, and /etc/systemd/system"
fi

# Light validation — better to fail here than after we've
# half-installed things.
if ! [[ "$DEVICE_ID" =~ ^[0-9a-fA-F]{8}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{12}$ ]]; then
    die "--device-id is not a valid UUID: $DEVICE_ID"
fi
if [[ "$OXY_URL" != http://* && "$OXY_URL" != https://* ]]; then
    die "--oxy-url must start with http:// or https://: $OXY_URL"
fi

# The Oxy API tree is mounted under `/api` (see crates/server/src/commands/serve.rs).
# The frontend SPA's fallback service catches any path that doesn't match a
# route and returns index.html with a 200 — so a missing /api turns into the
# worker getting HTML on every `/control/*` poll and choking on JSON parse.
# When the URL has *no* path component at all we know it's the common mistake
# and append `/api`; if the operator typed an intentional path (`/api/v2`,
# `/oxy-api`, etc.) we trust them. The strip is `:port`-aware so
# `host.example.com:3000` doesn't look like it has a path.
oxy_url_no_scheme="${OXY_URL#http://}"
oxy_url_no_scheme="${oxy_url_no_scheme#https://}"
if [[ "$oxy_url_no_scheme" != */* ]]; then
    OXY_URL="${OXY_URL%/}/api"
    warn "appended missing /api path to --oxy-url; final value: $OXY_URL"
    warn "pass an explicit path (e.g. /api/v2) to override this behaviour"
fi

# ── Step 1 — dependencies ────────────────────────────────────

log "checking dependencies"
command -v curl >/dev/null 2>&1 || die "curl is required (apt-get install curl)"

if ! command -v docker >/dev/null 2>&1; then
    if [[ "$SKIP_DOCKER_INSTALL" -eq 1 ]]; then
        die "docker not found and --skip-docker-install set"
    fi
    log "docker not found — installing via get.docker.com"
    curl -fsSL https://get.docker.com | sh
fi

if ! docker compose version >/dev/null 2>&1; then
    die "docker compose v2 plugin is required (try: apt-get install docker-compose-plugin)"
fi

# ── Step 2 — device identity ─────────────────────────────────

log "writing device identity to ${IDENTITY_DIR}/device.json"
mkdir -p "$IDENTITY_DIR"
chmod 755 "$IDENTITY_DIR"

# Reject silent identity rotation. Operators who actually want
# to repurpose a box and re-claim it elsewhere must pass --force.
if [[ -f "${IDENTITY_DIR}/device.json" && "$FORCE" -ne 1 ]]; then
    existing=$(grep -o '"device_id"[^,}]*' "${IDENTITY_DIR}/device.json" | head -1 || true)
    if [[ -n "$existing" && "$existing" != *"$DEVICE_ID"* ]]; then
        die "device.json already exists with a different device_id; use --force to overwrite"
    fi
fi

cat > "${IDENTITY_DIR}/device.json" <<JSON
{
  "device_id": "${DEVICE_ID}",
  "device_secret_b64": "${DEVICE_SECRET}",
  "sku": "${SKU}",
  "hardware_revision": "${HARDWARE_LABEL}"
}
JSON
chmod 600 "${IDENTITY_DIR}/device.json"

# ── Step 3 — compose + env ───────────────────────────────────

log "installing compose stack at ${INSTALL_ROOT}"
mkdir -p "$INSTALL_ROOT"
chmod 755 "$INSTALL_ROOT"

curl -fsSL "$COMPOSE_URL" -o "${INSTALL_ROOT}/docker-compose.yml"

# The Oxy API tree mounts under /api, so the MediaMTX-auth callback
# is /api/control/mtx-auth. Hard-coding the host-localhost dev default
# in docker-compose.yml caused the most recent production incident
# (boxes pointing at staging Oxy with --oxy-url, but coturn / MTX
# calling back to a non-existent local Oxy → 401 on every WHEP).
# Deriving from --oxy-url removes the foot-gun.
MTX_AUTH_URL="${OXY_URL%/}/control/mtx-auth"

# Keep the existing .env if one exists so operators who've
# tuned OUTBOX_PATH / EDGE_PUBLIC_HOST / etc. don't lose
# their work. We strip every var we're about to (re)write so
# re-running with new values doesn't leave a stale duplicate
# below the fresh one — dotenv files honor the last definition,
# but operators read top-down and get confused.
{
    if [[ -f "${INSTALL_ROOT}/.env" ]]; then
        # Lines we always rewrite: OXY_URL, MTX_AUTHHTTPADDRESS.
        # TS_AUTHKEY when fresh OR --clear-ts-authkey. TURN_AUTH_SECRET
        # whenever we're setting a secret OR --no-webrtc (the latter
        # writes a placeholder so compose's `${TURN_AUTH_SECRET:?...}`
        # passes).
        strip_re='^(OXY_URL|MTX_AUTHHTTPADDRESS)='
        if [[ -n "$TS_AUTHKEY" || "$CLEAR_TS_AUTHKEY" -eq 1 ]]; then
            strip_re="${strip_re}|^TS_AUTHKEY="
        fi
        if [[ -n "$TURN_AUTH_SECRET" || "$NO_WEBRTC" -eq 1 ]]; then
            strip_re="${strip_re}|^TURN_AUTH_SECRET="
        fi
        # Only strip ANTHROPIC_API_KEY when we're about to write a
        # new one. Operators who hand-edited the .env to add the
        # key shouldn't lose it on a re-install that omits the flag.
        if [[ -n "$ANTHROPIC_API_KEY" ]]; then
            strip_re="${strip_re}|^ANTHROPIC_API_KEY="
        fi
        # Same posture as ANTHROPIC_API_KEY: only rewrite UPSELL_CAMERAS when a
        # value is passed, so a re-install that omits the flag doesn't wipe an
        # operator's hand-set value.
        if [[ -n "$UPSELL_CAMERAS" ]]; then
            strip_re="${strip_re}|^UPSELL_CAMERAS="
        fi
        if [[ -n "$CONGESTION_CLIP_SEC" ]]; then
            strip_re="${strip_re}|^CONGESTION_CLIP_SEC="
        fi
        if [[ -n "$EDGE_ROLE" ]]; then
            strip_re="${strip_re}|^EDGE_ROLE="
        fi
        grep -vE "$strip_re" "${INSTALL_ROOT}/.env" || true
    fi
    printf 'OXY_URL=%s\n' "$OXY_URL"
    printf 'MTX_AUTHHTTPADDRESS=%s\n' "$MTX_AUTH_URL"
    if [[ -n "$TS_AUTHKEY" ]]; then
        printf 'TS_AUTHKEY=%s\n' "$TS_AUTHKEY"
    fi
    if [[ -n "$TURN_AUTH_SECRET" ]]; then
        printf 'TURN_AUTH_SECRET=%s\n' "$TURN_AUTH_SECRET"
    elif [[ "$NO_WEBRTC" -eq 1 ]]; then
        # Placeholder. coturn starts but its HMAC will never match
        # what Oxy mints, so WebRTC sessions silently fail at TURN auth.
        printf 'TURN_AUTH_SECRET=%s\n' "$NO_WEBRTC_PLACEHOLDER"
    fi
    if [[ -n "$ANTHROPIC_API_KEY" ]]; then
        printf 'ANTHROPIC_API_KEY=%s\n' "$ANTHROPIC_API_KEY"
    fi
    if [[ -n "$UPSELL_CAMERAS" ]]; then
        printf 'UPSELL_CAMERAS=%s\n' "$UPSELL_CAMERAS"
    fi
    if [[ -n "$CONGESTION_CLIP_SEC" ]]; then
        printf 'CONGESTION_CLIP_SEC=%s\n' "$CONGESTION_CLIP_SEC"
    fi
    if [[ -n "$EDGE_ROLE" ]]; then
        printf 'EDGE_ROLE=%s\n' "$EDGE_ROLE"
    fi
} > "${INSTALL_ROOT}/.env.new"
mv "${INSTALL_ROOT}/.env.new" "${INSTALL_ROOT}/.env"
chmod 600 "${INSTALL_ROOT}/.env"

if [[ "$CLEAR_TS_AUTHKEY" -eq 1 && -z "$TS_AUTHKEY" ]]; then
    log "--clear-ts-authkey set without a new --ts-authkey; TS_AUTHKEY removed from .env"
fi
if [[ "$NO_WEBRTC" -eq 1 ]]; then
    log "--no-webrtc set; TURN_AUTH_SECRET written as placeholder ('${NO_WEBRTC_PLACEHOLDER}') — WebRTC sessions will fail auth, HLS path remains functional"
fi

# ── Step 4 — systemd unit ────────────────────────────────────

log "installing systemd unit at ${SYSTEMD_PATH}"
curl -fsSL "$SYSTEMD_URL" -o "$SYSTEMD_PATH"
systemctl daemon-reload
systemctl enable "$SERVICE_NAME"
systemctl restart "$SERVICE_NAME"

# ── Step 5 — remote-support helpers (always installed, not run) ──
#
# Drop enable/disable scripts so the customer can opt in/out later
# even if they didn't pass --allow-remote-support at install. The
# scripts themselves are inert until the customer runs the enable
# one with a host auth key.
log "installing remote-support helpers under ${INSTALL_ROOT}"
curl -fsSL "$ENABLE_SUPPORT_URL"  -o "${INSTALL_ROOT}/enable-remote-support.sh"
curl -fsSL "$DISABLE_SUPPORT_URL" -o "${INSTALL_ROOT}/disable-remote-support.sh"
chmod 755 "${INSTALL_ROOT}/enable-remote-support.sh" "${INSTALL_ROOT}/disable-remote-support.sh"

# ── Step 6 — optionally enable remote support now ────────────
if [[ "$ALLOW_REMOTE_SUPPORT" -eq 1 ]]; then
    log "running enable-remote-support.sh (--allow-remote-support set)"
    "${INSTALL_ROOT}/enable-remote-support.sh" --authkey "$TS_HOST_AUTHKEY"
fi

# ── Step 7 — sanity tail ─────────────────────────────────────

log "service started; tailing journal for 10s"
timeout 10s journalctl -u "$SERVICE_NAME" -f --no-pager || true

cat <<DONE

✓ oxy-edge installed on $(hostname).

What just happened:
  • Identity:       ${IDENTITY_DIR}/device.json (0600)
  • Compose stack:  ${INSTALL_ROOT}/
  • Service:        ${SERVICE_NAME}
  • Remote support: $(if [[ "$ALLOW_REMOTE_SUPPORT" -eq 1 ]]; then echo "ENABLED (Oxy operators tagged tag:oxy-support can SSH)"; else echo "off — run ${INSTALL_ROOT}/enable-remote-support.sh to opt in"; fi)

Useful commands:
  systemctl status ${SERVICE_NAME}
  journalctl -u ${SERVICE_NAME} -f
  cd ${INSTALL_ROOT} && docker compose ps
  cd ${INSTALL_ROOT} && docker compose logs -f worker

The worker will announce + bootstrap against the pending claim
within ~5 minutes. Check the boxes table on ${OXY_URL} to confirm
the new device transitions from "waiting to bootstrap" to "active".
DONE
