#!/usr/bin/env bash
# end-to-end-demo.sh — reproduce the live smoke test from any clean machine.
#
# What it does:
#   1. Spawns a fresh /tmp git repo with one tracked file + one UNTRACKED file.
#   2. Fires a fake Claude Code Stop hook against lineage's HTTP endpoint.
#   3. Verifies the shadow ref landed in git.
#   4. Verifies the untracked file is in the snapshot tree (the bug that
#      disqualified `git stash create` is closed).
#   5. Deletes the untracked file from the working tree.
#   6. Runs `lineage::rewind_to` to restore it.
#
# Prerequisites (one terminal already running):
#   ./scripts/dev.sh
#
# Run this script in a separate terminal.
set -euo pipefail

HTTP_BASE="${LINEAGE_HTTP_BASE:-http://127.0.0.1:3211}"
WS_PORT="${WORKER_PORT:-49234}"
SMOKE="${SMOKE_DIR:-/tmp/lineage-demo}"

echo "==> setting up demo repo at $SMOKE"
rm -rf "$SMOKE"
mkdir -p "$SMOKE" && cd "$SMOKE"
git init -q -b main
git config user.email demo@example.com
git config user.name demo
echo "hello world" > README.md
git add . && git commit -qm "init"

# the magic: an untracked file. this is what an AI agent would create mid-session.
echo "secret-untracked content" > brand_new.txt
echo "    tracked:    README.md"
echo "    untracked:  brand_new.txt   <-- the file an agent just created"
echo

echo "==> ensuring lineage-strategy is pointing at the demo repo"
echo "    (LINEAGE_REPO_PATH=$SMOKE was set when dev.sh was launched, right?)"
echo

echo "==> POST /hook/claude-code/stop"
RESPONSE=$(curl -sS -X POST "$HTTP_BASE/hook/claude-code/stop" \
  -H 'Content-Type: application/json' \
  -d '{"session_id":"demo-1","hook_event_name":"Stop"}')
echo "    $RESPONSE"
echo

echo "==> waiting for FIFO queue to consume snapshot..."
sleep 1.5

echo "==> shadow refs in $SMOKE:"
git -C "$SMOKE" for-each-ref refs/iii/lineage/checkpoints/v0/

REF=$(git -C "$SMOKE" for-each-ref refs/iii/lineage/checkpoints/v0/demo-1/ --format='%(refname)' | head -1)
if [ -z "$REF" ]; then
  echo "ERROR: no shadow ref landed. Check data/logs/lineage-gitops.log for the gitops worker output." >&2
  exit 1
fi
echo

echo "==> tree at $REF (untracked file should be present):"
git -C "$SMOKE" ls-tree -r "$REF"
echo

echo "==> deleting brand_new.txt..."
rm "$SMOKE/brand_new.txt"
ls "$SMOKE"
echo

echo "==> lineage::rewind_to $REF"
iii trigger --port "$WS_PORT" --function-id "lineage::rewind_to" \
  --payload "{\"repo_path\":\"$SMOKE\",\"shadow_ref\":\"$REF\",\"force\":true}"
echo

echo "==> after rewind:"
ls "$SMOKE"
if [ -f "$SMOKE/brand_new.txt" ]; then
  echo "    brand_new.txt restored. content: $(cat "$SMOKE/brand_new.txt")"
  echo
  echo "==> SMOKE PASS"
else
  echo "    ERROR: brand_new.txt missing. Rewind did not restore the untracked file." >&2
  exit 1
fi
