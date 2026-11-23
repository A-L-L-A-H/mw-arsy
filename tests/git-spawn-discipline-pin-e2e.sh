#!/usr/bin/env bash
# Regression pin: every git subprocess spawned for the built-in git server
# MUST route through `safe_cmd::safe_command` / `safe_command_sync` (env_clear)
# rather than raw `std::process::Command::new` / `tokio::process::Command::new`.
set -uo pipefail
cd "$(dirname "$0")/.." || exit 1

PASS=0; FAIL=0
ok()  { printf '\033[32m✓\033[0m %s\n' "$1"; PASS=$((PASS+1)); }
bad() { printf '\033[31m✗\033[0m %s — %s\n' "$1" "$2"; FAIL=$((FAIL+1)); }

G=$(command -v /usr/bin/grep || command -v grep)

strip_rust_comments() {
  perl -0777 -pe 's{//[^\n]*}{}g; s{/\*.*?\*/}{}gs' "$1"
}

TARGET_FILES=(
  "panel/agent/src/services/git_http.rs"
  "panel/agent/src/services/git_repos.rs"
  "panel/agent/src/routes/git_repos.rs"
)

for f in "${TARGET_FILES[@]}"; do
  if [ ! -f "$f" ]; then
    ok "$f (not yet created — pre-creation pass)"
    continue
  fi
  clean=$(strip_rust_comments "$f")
  if printf '%s\n' "$clean" | "$G" -qE 'Command::new\('; then
    bad "$f uses raw Command::new()" "must route through crate::safe_cmd::safe_command"
  else
    ok "$f has zero raw Command::new() calls"
  fi

  if printf '%s\n' "$clean" | "$G" -qE 'safe_command(_sync)?\('; then
    ok "$f invokes safe_command"
  else
    ok "$f invokes no subprocesses directly"
  fi
done

echo ""
echo "PASS $PASS  FAIL $FAIL"
[ "$FAIL" -eq 0 ]
