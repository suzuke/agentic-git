#!/usr/bin/env bash
#
# verify.sh — the deterministic SUPERVISOR for the multi-agent scenario. It owns
# setup and synthesis; the agents only execute + produce evidence (see
# agent-run.sh). Two real guarded agent sessions run CONCURRENTLY against ONE
# shared repo, each on its own branch; then the supervisor re-derives the truth
# from git/home/audit STATE (never from an agent's own verdict) and checks the
# multi-agent invariants. An agent that reports PASS but whose state disagrees
# still FAILS.
#
#   ./verify.sh            # run it, print the synthesis
#   ./verify.sh --keep     # keep the throwaway world for inspection
#
set -u
unset AGENTIC_GIT_BYPASS AGEND_GIT_BYPASS \
      AGENTIC_GIT_BYPASS_AGENT AGEND_GIT_BYPASS_AGENT \
      AGENTIC_GIT_BYPASS_UNTIL AGEND_GIT_BYPASS_UNTIL \
      AGENTIC_GIT_AGENT AGENTIC_GIT_HOME 2>/dev/null || true

here="$(cd "$(dirname "$0")" && pwd)"
KEEP=0; [ "${1:-}" = "--keep" ] && KEEP=1
c() { printf '\033[%sm' "$1"; }
say()  { printf '\n%s▸ %s%s\n' "$(c '1;36')" "$*" "$(c 0)"; }
pass() { printf '  %s✓ %s%s\n' "$(c '1;32')" "$*" "$(c 0)"; }
bad()  { printf '  %s✗ %s%s\n' "$(c '1;31')" "$*" "$(c 0)"; FAILS=$((FAILS+1)); }
die()  { printf '\n%s✗ %s%s\n' "$(c '1;31')" "$*" "$(c 0)" >&2; exit 1; }

# agentic-git: explicit override, then installed, then the repo's release build.
BIN="${AGENTIC_GIT_BIN:-}"
{ [ -n "$BIN" ] && [ -x "$BIN" ]; } || BIN="$(command -v agentic-git || true)"
repo_root="$(cd "$here/../.." 2>/dev/null && pwd || true)"
if { [ -z "$BIN" ] || [ ! -x "$BIN" ]; } && [ -n "$repo_root" ] && [ -f "$repo_root/Cargo.toml" ]; then
  BIN="$repo_root/target/release/agentic-git"
  [ -x "$BIN" ] || ( cd "$repo_root" && cargo build --release -p agentic-git >/dev/null ) || die "build failed"
fi
{ [ -n "$BIN" ] && [ -x "$BIN" ]; } || die "agentic-git not found — cargo install agentic-git, or run from the repo"

resolve_real_git() { local d IFS=:; for d in $PATH; do case "$d" in *.agentic-git*|*.agend-terminal*|*/.cargo/bin*) continue ;; esac; [ -x "$d/git" ] && { printf '%s\n' "$d/git"; return 0; }; done; return 1; }
REAL_GIT="$(resolve_real_git)" || die "no real (non-shim) git on PATH"
sanitized_path() { local d IFS=: out; out="$(dirname "$REAL_GIT")"; for d in $PATH; do case "$d" in *.agentic-git*|*.agend-terminal*) continue ;; esac; out="$out:$d"; done; printf '%s\n' "$out"; }

# ── the shared world ─────────────────────────────────────────────────────────
work="$(mktemp -d)"; [ "$KEEP" = 1 ] || trap 'rm -rf "$work"' EXIT
project="$work/project"; canonical="$work/your-checkout"; bare="$work/origin.git"; home="$work/home"
arts="$work/artifacts"; mkdir -p "$project" "$canonical" "$arts/a" "$arts/b"
export GIT_AUTHOR_NAME=you GIT_AUTHOR_EMAIL=you@example.com GIT_COMMITTER_NAME=you GIT_COMMITTER_EMAIL=you@example.com
"$REAL_GIT" init -q --bare "$bare"
"$REAL_GIT" -C "$project" init -q -b main
"$REAL_GIT" -C "$project" config user.name you; "$REAL_GIT" -C "$project" config user.email you@example.com
"$REAL_GIT" -C "$project" remote add origin "$bare"
printf 'shared project\n' > "$project/README.md"
"$REAL_GIT" -C "$project" add -A; "$REAL_GIT" -C "$project" commit -qm "project baseline"
"$REAL_GIT" -C "$project" push -q origin main
PROJECT_BASE="$("$REAL_GIT" -C "$project" rev-parse HEAD)"
"$REAL_GIT" -C "$canonical" init -q -b main
"$REAL_GIT" -C "$canonical" config user.name you; "$REAL_GIT" -C "$canonical" config user.email you@example.com
"$REAL_GIT" -C "$canonical" remote add origin https://example.invalid/your-project.git
printf 'your real work\n' > "$canonical/app.py"; "$REAL_GIT" -C "$canonical" add -A; "$REAL_GIT" -C "$canonical" commit -qm base
"$REAL_GIT" -C "$canonical" commit -q --allow-empty -m more

spawn_agent() {  # spawn_agent <role> <artifact-dir>
  local role="$1" art="$2" other
  [ "$role" = a ] && other=b || other=a
  ( cd "$project" && \
    AGENTIC_GIT_HOME="$home" AGENTIC_GIT_REAL_GIT="$REAL_GIT" AGENTIC_GIT_BIN="$BIN" \
    PATH="$(sanitized_path)" \
    "$BIN" run --agent "agent-$role" --branch "feat/$role" -- \
      sh "$here/agent-run.sh" "agent-$role" "feat/$role" "feat/$other" "$art" "$canonical" \
  ) >"$art/session.log" 2>&1
}

say "Launching agent-a and agent-b CONCURRENTLY against one shared repo…"
spawn_agent a "$arts/a" &  pa=$!
spawn_agent b "$arts/b" &  pb=$!
wait "$pa"; ra=$?
wait "$pb"; rb=$?
[ "$ra" = 0 ] || printf '  (agent-a session exit %s — see %s)\n' "$ra" "$arts/a/session.log"
[ "$rb" = 0 ] || printf '  (agent-b session exit %s — see %s)\n' "$rb" "$arts/b/session.log"

# ── SYNTHESIS: re-derive the truth from STATE, not from the agents' word ──────
say "Synthesis — re-deriving the truth from git/home/audit state:"
FAILS=0
ev() { grep "^$2=" "$arts/$1/evidence.env" 2>/dev/null | head -1 | cut -d= -f2-; }

# S1 two distinct, valid worktrees, each on its own branch
wa="$(ev a WORKTREE)"; wb="$(ev b WORKTREE)"; ba="$(ev a BRANCH)"; bb="$(ev b BRANCH)"
{ [ -n "$wa" ] && [ -n "$wb" ] && [ "$wa" != "$wb" ]; } && pass "two distinct worktrees ($ba, $bb)" || bad "worktrees not distinct: '$wa' vs '$wb'"
{ [ "$ba" = feat/a ] && [ "$bb" = feat/b ]; } && pass "each agent on its own bound branch" || bad "branch binding wrong: a=$ba b=$bb"

# S2 git resolved to the shim inside BOTH sessions (not the real/outer git)
{ case "$(ev a GITBIN)" in */bin/git) true;; *) false;; esac && case "$(ev b GITBIN)" in */bin/git) true;; *) false;; esac; } \
  && pass "both agents' git resolved to the shim ($(ev a GITBIN))" || bad "an agent's git was NOT the shim (a=$(ev a GITBIN) b=$(ev b GITBIN))"

# S3 two branches on the shared origin, distinct tips
ta="$("$REAL_GIT" -C "$bare" rev-parse feat/a 2>/dev/null)"; tb="$("$REAL_GIT" -C "$bare" rev-parse feat/b 2>/dev/null)"
{ [ -n "$ta" ] && [ -n "$tb" ] && [ "$ta" != "$tb" ]; } && pass "both branches pushed to the shared origin, distinct" || bad "origin branches missing/equal (a=$ta b=$tb)"

# S4 provenance on the origin: each branch's tip is trailered to its OWN agent
"$REAL_GIT" -C "$bare" log -1 --format=%B feat/a 2>/dev/null | grep -q "Agentic-Agent: agent-a" && pa_ok=1 || pa_ok=0
"$REAL_GIT" -C "$bare" log -1 --format=%B feat/b 2>/dev/null | grep -q "Agentic-Agent: agent-b" && pb_ok=1 || pb_ok=0
"$REAL_GIT" -C "$bare" log -1 --format=%B feat/a 2>/dev/null | grep -q "Agentic-Agent: agent-b" && cross=1 || cross=0
{ [ "$pa_ok" = 1 ] && [ "$pb_ok" = 1 ] && [ "$cross" = 0 ]; } && pass "provenance is per-agent and not mixed" || bad "provenance wrong (a=$pa_ok b=$pb_ok cross=$cross)"

# S5 no cross-contamination: each worktree has its own file, NOT the other's
{ [ -f "$wa/solver_agent-a.py" ] && [ ! -f "$wa/solver_agent-b.py" ] \
  && [ -f "$wb/solver_agent-b.py" ] && [ ! -f "$wb/solver_agent-a.py" ]; } \
  && pass "agents' working trees are isolated (no cross-contamination)" || bad "cross-contamination between worktrees"

# S6 the shared source repo was untouched (agents worked in worktrees)
now="$("$REAL_GIT" -C "$project" rev-parse HEAD)"; cur="$("$REAL_GIT" -C "$project" rev-parse --abbrev-ref HEAD)"
{ [ "$now" = "$PROJECT_BASE" ] && [ "$cur" = main ]; } && pass "the shared source repo's HEAD is untouched" || bad "source repo drifted (HEAD=$now branch=$cur)"

# S7 one integrity key, both bindings present (no split-brain)
ks=$(wc -c < "$home/.config-integrity-key" 2>/dev/null | tr -d ' ')
{ [ "$ks" = 32 ] && [ -f "$home/runtime/agent-a/binding.json" ] && [ -f "$home/runtime/agent-a/binding.json.sig" ] \
  && [ -f "$home/runtime/agent-b/binding.json" ] && [ -f "$home/runtime/agent-b/binding.json.sig" ]; } \
  && pass "one 32-byte key; both agents' signed bindings present" || bad "key/binding split-brain (keysize=$ks)"

# S8 every graded step's real exit code matches its expectation (re-checked from
#    the raw rc files, not from the agent's verdict) — for BOTH agents
step_state_ok() { local role="$1" n exp rc bad2=0
  for n in $(seq 1 10); do
    exp=$(cat "$arts/$role/$n.expect" 2>/dev/null); rc=$(cat "$arts/$role/$n.rc" 2>/dev/null)
    case "$exp" in
      ok)   [ "$rc" = 0 ]  || { bad2=1; printf '      agent-%s step %s expected ok, got rc=%s\n' "$role" "$n" "$rc"; } ;;
      deny) [ "$rc" != 0 ] || { bad2=1; printf '      agent-%s step %s expected DENY, got rc=%s (guard did not fire!)\n' "$role" "$n" "$rc"; } ;;
      *)    bad2=1 ;;
    esac
  done
  return $bad2
}
step_state_ok a && sa=1 || sa=0
step_state_ok b && sb=1 || sb=0
{ [ "$sa" = 1 ] && [ "$sb" = 1 ]; } && pass "every guarded step behaved as required (own push ok; cross-agent ops denied)" || bad "a guarded step misbehaved"

# S9 each agent's SELF-verdict must be PASS AND agree with the re-derived state
va=$(cat "$arts/a/verdict.txt" 2>/dev/null); vb=$(cat "$arts/b/verdict.txt" 2>/dev/null)
{ [ "$va" = PASS ] && [ "$sa" = 1 ]; } && pass "agent-a self-verdict PASS and consistent with state" || bad "agent-a verdict '$va' inconsistent with state"
{ [ "$vb" = PASS ] && [ "$sb" = 1 ]; } && pass "agent-b self-verdict PASS and consistent with state" || bad "agent-b verdict '$vb' inconsistent with state"

say "$([ "$FAILS" = 0 ] && printf '%sMULTI-AGENT SCENARIO VERIFIED%s — %d invariants held; two agents shared one repo, stayed isolated, kept their provenance, and could not clobber each other.' "$(c '1;32')" "$(c 0)" 9 || printf '%sFAILED%s — %d invariant(s) did not hold.' "$(c '1;31')" "$(c 0)" "$FAILS")"
[ "$KEEP" = 1 ] && printf '\n  world kept at: %s\n' "$work"
exit "$([ "$FAILS" = 0 ] && echo 0 || echo 1)"
