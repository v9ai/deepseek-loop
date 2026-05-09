#!/usr/bin/env bash
# Publish deepseek-loop to crates.io using the CRATES_IO_TOKEN held in
# Cloudflare Secrets Store. The store does not return secret values via REST
# API, so we deploy a one-shot Worker bound to the secret, fetch the value
# over HTTPS guarded by a random query param, then delete the Worker.
#
# Usage: ./scripts/publish.sh
#
# Preconditions:
#   * `wrangler` logged in to the Cloudflare account that owns the Secrets
#     Store entry (OAuth creds in ~/Library/Preferences/.wrangler/config/).
#   * No `CLOUDFLARE_API_TOKEN` env var with insufficient scopes — the script
#     unsets it locally to force OAuth.
#   * Working tree clean, on `main`, version in Cargo.toml not yet on
#     crates.io.
#
# Side effects on the user's machine: temporarily writes
# ~/.cargo/credentials.toml (cleared on exit) and creates/deletes a temp
# Cloudflare Worker named `cargo-token-fetch-temp`.

set -euo pipefail

CF_STORE_ID="ec928f4771fb4577a607a0b122e8087e"
CF_SECRET_NAME="CRATES_IO_TOKEN"
WORKER_NAME="cargo-token-fetch-temp"
FEATURES="scheduler,builtin-tools,reqwest-client,cache,cli"

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$ROOT"

color() { printf '\033[1;36m%s\033[0m\n' "$*"; }
warn()  { printf '\033[1;33m%s\033[0m\n' "$*" >&2; }
fail()  { printf '\033[1;31m%s\033[0m\n' "$*" >&2; exit 1; }

# Pre-flight: clean tree + sane branch.
if [ -n "$(git status --porcelain)" ]; then
    fail "publish: working tree is dirty; commit or stash first"
fi

VERSION="$(awk -F'"' '/^version =/ { print $2; exit }' Cargo.toml)"
[ -n "$VERSION" ] || fail "publish: could not parse version from Cargo.toml"
color "publish: deepseek-loop v${VERSION}"

# Refuse to publish a version that already exists.
if cargo search deepseek-loop --limit 1 2>/dev/null | grep -qE "^deepseek-loop = \"${VERSION}\""; then
    fail "publish: v${VERSION} is already on crates.io — bump the version first"
fi

# Verify gate.
color "publish: cargo fmt --check"
cargo fmt --check
color "publish: cargo test --features $FEATURES"
cargo test --features "$FEATURES" --quiet

# Scaffold temp Worker.
TMPDIR_RUN="$(mktemp -d -t cargo-token-fetch-XXXXXX)"
GUARD="$(openssl rand -hex 24)"
WORKER_DEPLOYED=0
LOGGED_IN=0

cleanup() {
    local rc=$?
    set +e
    if [ "$LOGGED_IN" -eq 1 ]; then
        cargo logout >/dev/null 2>&1 || true
        rm -f "$HOME/.cargo/credentials.toml" 2>/dev/null || true
    fi
    if [ "$WORKER_DEPLOYED" -eq 1 ]; then
        ( cd "$TMPDIR_RUN" && CLOUDFLARE_API_TOKEN= wrangler delete --name "$WORKER_NAME" >/dev/null 2>&1 ) || \
            warn "cleanup: could not delete Worker '$WORKER_NAME' — delete manually via dash"
    fi
    if [ -d "$TMPDIR_RUN" ]; then
        find "$TMPDIR_RUN" -type f -delete 2>/dev/null || true
        find "$TMPDIR_RUN" -type d -empty -delete 2>/dev/null || true
    fi
    exit $rc
}
trap cleanup EXIT

mkdir -p "$TMPDIR_RUN/src"
cat > "$TMPDIR_RUN/wrangler.toml" <<EOF
name = "$WORKER_NAME"
main = "src/index.js"
compatibility_date = "2025-05-01"

[[secrets_store_secrets]]
binding = "TOK"
store_id = "$CF_STORE_ID"
secret_name = "$CF_SECRET_NAME"
EOF
cat > "$TMPDIR_RUN/src/index.js" <<EOF
export default {
  async fetch(request, env) {
    const url = new URL(request.url);
    if (url.searchParams.get("k") !== "$GUARD") {
      return new Response("nope", { status: 403 });
    }
    const v = await env.TOK.get();
    return new Response(v, { headers: { "content-type": "text/plain" } });
  }
};
EOF

# Deploy. The local CLOUDFLARE_API_TOKEN env var commonly lacks
# secrets_store scope (see lead-gen/.env.local), so unset it for this run.
color "publish: wrangler deploy ($WORKER_NAME)"
DEPLOY_OUT="$(cd "$TMPDIR_RUN" && CLOUDFLARE_API_TOKEN= wrangler deploy 2>&1)"
WORKER_DEPLOYED=1
WORKER_URL="$(printf '%s\n' "$DEPLOY_OUT" | awk '/https:\/\/.*workers\.dev/ { print $1; exit }')"
[ -n "$WORKER_URL" ] || { printf '%s\n' "$DEPLOY_OUT" >&2; fail "publish: could not parse Worker URL from deploy output"; }
color "publish: worker live at $WORKER_URL"

# Fetch the token. Workers can take a beat to propagate after deploy.
TOKEN=""
for attempt in 1 2 3 4 5 6 7 8; do
    sleep 2
    body="$(curl -fsS "${WORKER_URL}/?k=${GUARD}" 2>/dev/null || true)"
    if [ -n "$body" ] && [ "$body" != "nope" ]; then
        TOKEN="$body"
        break
    fi
    warn "publish: token fetch attempt ${attempt} empty; retrying..."
done
[ -n "$TOKEN" ] || fail "publish: could not retrieve token from Worker after 8 attempts"

# Login + publish.
color "publish: cargo login"
printf '%s' "$TOKEN" | cargo login --quiet
LOGGED_IN=1
unset TOKEN

color "publish: cargo publish --features cli"
cargo publish --features cli

color "publish: deepseek-loop v${VERSION} uploaded to crates.io"
