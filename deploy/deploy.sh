#!/usr/bin/env bash
#
# Deploy this repo to the LXC container that serves demo.ovk.zero2one.ee.
#
# Everything the deploy needs is either in this file or passed in as
# environment. No personal ssh-config aliases, no tailnet, no interactive
# prompts. The one thing that is deliberately NOT here is the SSH key: it
# lives in GitHub Actions secrets (or in your own agent when running locally),
# because a key committed to a repo is a key given to everyone who ever
# cloned it.
#
# Usage, locally (uses whatever key your ssh-agent already holds):
#   ./deploy/deploy.sh
#
# Usage, with an explicit key:
#   DEPLOY_SSH_KEY_FILE=~/.ssh/ovk_deploy ./deploy/deploy.sh
#
# In CI: .github/workflows/deploy.yml writes the secret to a file and calls
# this script. The script is the single source of truth for how a deploy
# happens; the workflow only supplies credentials.

set -euo pipefail

DEPLOY_HOST="${DEPLOY_HOST:-46.4.94.200}"
DEPLOY_USER="${DEPLOY_USER:-root}"
DEPLOY_CONTAINER="${DEPLOY_CONTAINER:-ovk-demo}"
DEPLOY_PATH="${DEPLOY_PATH:-/opt/ovk-prototype}"
DEPLOY_SERVICE="${DEPLOY_SERVICE:-ovk-demo}"
DEPLOY_HEALTH_URL="${DEPLOY_HEALTH_URL:-https://demo.ovk.zero2one.ee/mockup}"
DEPLOY_REF="${DEPLOY_REF:-HEAD}"

say() { printf '\n\033[1m==> %s\033[0m\n' "$*"; }
die() { printf '\n\033[31merror: %s\033[0m\n' "$*" >&2; exit 1; }

command -v git >/dev/null || die "git not found"
git rev-parse --git-dir >/dev/null 2>&1 || die "not inside the git repository"

REPO_ROOT="$(git rev-parse --show-toplevel)"
cd "$REPO_ROOT"

COMMIT="$(git rev-parse "$DEPLOY_REF")"
COMMIT_SHORT="$(git rev-parse --short "$DEPLOY_REF")"
SUBJECT="$(git log -1 --format=%s "$DEPLOY_REF")"

# Deploy the committed tree, never the working tree: what ships is exactly
# what is in git, with no stray local edits and no .git directory.
if ! git diff --quiet || ! git diff --cached --quiet; then
  echo "note: working tree has uncommitted changes; they will NOT be deployed."
fi

SSH_OPTS=(
  -o BatchMode=yes
  -o StrictHostKeyChecking=accept-new
  -o ConnectTimeout=20
  -o ServerAliveInterval=30
)
[ -n "${DEPLOY_SSH_KEY_FILE:-}" ] && SSH_OPTS+=(-i "$DEPLOY_SSH_KEY_FILE" -o IdentitiesOnly=yes)

TARGET="${DEPLOY_USER}@${DEPLOY_HOST}"
TARBALL="$(mktemp -t ovk-deploy.XXXXXX).tar"
trap 'rm -f "$TARBALL"' EXIT

say "Deploying ${COMMIT_SHORT} (${SUBJECT}) to ${DEPLOY_CONTAINER} on ${DEPLOY_HOST}"

say "Packing committed tree"
git archive --format=tar "$DEPLOY_REF" -o "$TARBALL"
echo "$(wc -c < "$TARBALL" | tr -d ' ') bytes"

say "Checking host and container"
ssh "${SSH_OPTS[@]}" "$TARGET" "bash -s" <<REMOTE_CHECK || die "host/container preflight failed"
set -euo pipefail
command -v lxc >/dev/null || { echo "lxc not found on host"; exit 1; }
sudo -n true 2>/dev/null || { echo "passwordless sudo unavailable"; exit 1; }
state=\$(sudo -n lxc list "$DEPLOY_CONTAINER" -c s --format csv 2>/dev/null | head -1)
[ "\$state" = "RUNNING" ] || { echo "container $DEPLOY_CONTAINER is '\$state', expected RUNNING"; exit 1; }
echo "host ok, container $DEPLOY_CONTAINER RUNNING"
REMOTE_CHECK

say "Uploading"
scp "${SSH_OPTS[@]}" "$TARBALL" "$TARGET:/tmp/ovk-deploy.tar" >/dev/null
ssh "${SSH_OPTS[@]}" "$TARGET" "sudo -n lxc file push /tmp/ovk-deploy.tar ${DEPLOY_CONTAINER}/tmp/ovk-deploy.tar" 2>/dev/null

say "Building and restarting inside the container"
ssh "${SSH_OPTS[@]}" "$TARGET" \
  "CONTAINER='$DEPLOY_CONTAINER' APPPATH='$DEPLOY_PATH' SERVICE='$DEPLOY_SERVICE' COMMIT='$COMMIT' bash -s" <<'REMOTE_DEPLOY'
set -euo pipefail

# `lxc exec` reads stdin, which would eat the rest of this script as it is fed
# to `bash -s` over ssh. Always give it /dev/null instead.
run() { sudo -n lxc exec "$CONTAINER" -- bash -lc "$1" < /dev/null; }

# Keep the previous binary so a bad build can be put back by hand.
run "mkdir -p '$APPPATH' && cp -a '$APPPATH/server/target/release/ovk-prototype-server' /tmp/ovk-binary-previous 2>/dev/null || true"

run "cd '$APPPATH' && tar xf /tmp/ovk-deploy.tar && printf '%s\n' '$COMMIT' > .deployed-commit"

echo "--- cargo build --release ---"
run "cd '$APPPATH/server' && source \$HOME/.cargo/env 2>/dev/null || true; cargo build --release 2>&1 | tail -5"

run "systemctl restart '$SERVICE'"
sleep 3
run "systemctl is-active '$SERVICE'" || { echo 'service did not come back active'; run "journalctl -u '$SERVICE' -n 30 --no-pager"; exit 1; }

run "rm -f /tmp/ovk-deploy.tar"
REMOTE_DEPLOY

ssh "${SSH_OPTS[@]}" "$TARGET" "rm -f /tmp/ovk-deploy.tar"

say "Verifying"
code="$(curl -s -o /dev/null -w '%{http_code}' --max-time 30 "$DEPLOY_HEALTH_URL" || true)"
[ "$code" = "200" ] || die "health check on $DEPLOY_HEALTH_URL returned $code"

deployed="$(ssh "${SSH_OPTS[@]}" "$TARGET" \
  "sudo -n lxc exec $DEPLOY_CONTAINER -- cat $DEPLOY_PATH/.deployed-commit < /dev/null" 2>/dev/null \
  | tr -d '[:space:]' || true)"
[ "$deployed" = "$COMMIT" ] || die "container reports '${deployed:-nothing}', expected $COMMIT"

say "Deployed ${COMMIT_SHORT} — ${DEPLOY_HEALTH_URL} returned 200"
