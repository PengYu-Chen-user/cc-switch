#!/usr/bin/env bash
#
# Upstream auto-sync for the cc-switch fork.
#
# Mirrors upstream/main into this fork's main, then merges main into the
# feature branch that carries the extra Kimi Code / DeepSeek Harness support.
# Pauses itself (marker file + issue) on breaking upstream changes or when
# upstream already ships the same support.
#
# Environment (set by .github/workflows/sync-upstream.yml):
#   UPSTREAM_REPO, MAIN_BRANCH, FEATURE_BRANCH, BASE_SCHEMA_VERSION,
#   FORCE, RUN_BACKEND_CHECKS, GH_TOKEN, GITHUB_REPOSITORY

set -euo pipefail

UPSTREAM_REPO="${UPSTREAM_REPO:-farion1231/cc-switch}"
MAIN_BRANCH="${MAIN_BRANCH:-main}"
FEATURE_BRANCH="${FEATURE_BRANCH:-feat/kimicode-dsh}"
BASE_SCHEMA_VERSION="${BASE_SCHEMA_VERSION:-18}"
FORCE="${FORCE:-false}"
RUN_BACKEND_CHECKS="${RUN_BACKEND_CHECKS:-true}"
PAUSE_FILE=".github/sync-upstream.paused"

git config user.name "github-actions[bot]"
git config user.email "41898282+github-actions[bot]@users.noreply.github.com"

if git remote get-url upstream >/dev/null 2>&1; then
  git remote set-url upstream "https://github.com/${UPSTREAM_REPO}.git"
else
  git remote add upstream "https://github.com/${UPSTREAM_REPO}.git"
fi

git fetch --no-tags origin "+refs/heads/*:refs/remotes/origin/*"
git fetch --no-tags upstream "+refs/heads/*:refs/remotes/upstream/*"

ensure_issue() {
  local reason="$1"
  gh label create sync-upstream --repo "$GITHUB_REPOSITORY" \
    --color BFD4F2 --description "Upstream sync automation" >/dev/null 2>&1 || true

  local open_count
  open_count="$(gh issue list --repo "$GITHUB_REPOSITORY" --state open \
    --label sync-upstream --json number --jq 'length' 2>/dev/null || echo 0)"

  if [ "${open_count:-0}" = "0" ]; then
    gh issue create --repo "$GITHUB_REPOSITORY" \
      --title "Upstream auto-sync paused: ${reason}" \
      --label sync-upstream \
      --body "$(cat <<EOF
The scheduled upstream sync paused itself and will not run again until resumed.

**Reason:** ${reason}

**What to do**
- If this is a **breaking upstream change**, reconcile \`${FEATURE_BRANCH}\` by hand (resolve the conflict / DB schema bump), then resume.
- If **upstream now ships the same support**, retire the fork's extra patches (keep only what upstream still lacks) or delete this workflow.
- Otherwise, once resolved, run the **Sync upstream** workflow manually with \`force: true\` to clear the pause marker and resume.

_State marker: \`.github/sync-upstream.paused\` on \`${MAIN_BRANCH}\`._
EOF
)" >/dev/null 2>&1 || true
  fi
}

pause() {
  local reason="$1"
  echo "::warning::Upstream auto-sync paused: ${reason}"
  git merge --abort >/dev/null 2>&1 || true
  git checkout -f "$MAIN_BRANCH"
  git reset --hard "origin/${MAIN_BRANCH}"
  mkdir -p "$(dirname "$PAUSE_FILE")"
  printf '%s\n' "$reason" > "$PAUSE_FILE"
  git add "$PAUSE_FILE"
  if ! git diff --cached --quiet; then
    git commit -m "chore(sync): pause upstream auto-sync (${reason})"
    git push origin "HEAD:${MAIN_BRANCH}"
  fi
  ensure_issue "$reason"
  exit 0
}

# ── Pause state ──────────────────────────────────────────────────────────────
if git cat-file -e "origin/${MAIN_BRANCH}:${PAUSE_FILE}" 2>/dev/null; then
  if [ "$FORCE" != "true" ]; then
    echo "Auto-sync is paused: $(git show "origin/${MAIN_BRANCH}:${PAUSE_FILE}")"
    echo "Re-run this workflow with force=true to resume."
    exit 0
  fi
  echo "Force mode: attempting to resume."
fi

# ── Guard: upstream already added the same support ───────────────────────────
if git cat-file -e "upstream/${MAIN_BRANCH}:src-tauri/src/kimicode_config.rs" 2>/dev/null \
  || git cat-file -e "upstream/${MAIN_BRANCH}:src-tauri/src/dsh_config.rs" 2>/dev/null \
  || git show "upstream/${MAIN_BRANCH}:src-tauri/src/app_config.rs" 2>/dev/null \
       | grep -qE '\b(Kimicode|Dsh)\b'; then
  pause "upstream/main already supports Kimi Code (kimicode) and/or DeepSeek Harness (dsh)"
fi

# ── Guard: upstream DB schema advanced past our fork base ────────────────────
UPSTREAM_SCHEMA="$(git show "upstream/${MAIN_BRANCH}:src-tauri/src/database/mod.rs" 2>/dev/null \
  | grep -oE 'SCHEMA_VERSION: i32 = [0-9]+' | grep -oE '[0-9]+' | head -n1 || true)"
if [ -n "${UPSTREAM_SCHEMA}" ] && [ "${UPSTREAM_SCHEMA}" -gt "${BASE_SCHEMA_VERSION}" ]; then
  pause "upstream database schema advanced to v${UPSTREAM_SCHEMA} (base v${BASE_SCHEMA_VERSION}); our v18->v19 migration needs manual reconciliation"
fi

# ── Sync main ────────────────────────────────────────────────────────────────
git checkout -B "$MAIN_BRANCH" "origin/${MAIN_BRANCH}"
if ! git merge --no-edit "upstream/${MAIN_BRANCH}"; then
  git merge --abort >/dev/null 2>&1 || true
  pause "merge conflict while syncing ${MAIN_BRANCH} with upstream/${MAIN_BRANCH}"
fi
git push origin "${MAIN_BRANCH}:${MAIN_BRANCH}"

# ── Merge main into the feature branch ───────────────────────────────────────
git checkout -B "$FEATURE_BRANCH" "origin/${FEATURE_BRANCH}"
if ! git merge --no-edit "$MAIN_BRANCH"; then
  git merge --abort >/dev/null 2>&1 || true
  pause "merge conflict updating ${FEATURE_BRANCH} from ${MAIN_BRANCH} (breaking upstream change)"
fi

# ── Post-merge checks (breakage detector) ────────────────────────────────────
run_checks() {
  local rc=0
  mkdir -p dist
  pnpm install --frozen-lockfile || rc=1
  pnpm typecheck || rc=1
  pnpm format:check || rc=1
  pnpm test:unit || rc=1
  if [ "$RUN_BACKEND_CHECKS" = "true" ]; then
    cargo fmt --check --manifest-path src-tauri/Cargo.toml || rc=1
    cargo clippy --manifest-path src-tauri/Cargo.toml -- -D warnings || rc=1
    cargo test --manifest-path src-tauri/Cargo.toml || rc=1
  fi
  return $rc
}

if ! run_checks; then
  pause "post-merge checks failed (frontend/backend build or tests are red)"
fi

# ── Push the updated feature branch ──────────────────────────────────────────
git push origin "${FEATURE_BRANCH}:${FEATURE_BRANCH}"

# ── Resume: clear the pause marker after a clean force-run ───────────────────
if git cat-file -e "origin/${MAIN_BRANCH}:${PAUSE_FILE}" 2>/dev/null; then
  git checkout -f "$MAIN_BRANCH"
  git pull --ff-only origin "$MAIN_BRANCH" || true
  if [ -f "$PAUSE_FILE" ]; then
    git rm -f "$PAUSE_FILE" >/dev/null 2>&1 || rm -f "$PAUSE_FILE"
  fi
  if ! git diff --cached --quiet || ! git diff --quiet; then
    git add -A
    git commit -m "chore(sync): resume upstream auto-sync"
    git push origin "${MAIN_BRANCH}:${MAIN_BRANCH}"
  fi
fi

echo "Upstream sync finished cleanly."
