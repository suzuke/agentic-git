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
CANON_BASE="$("$REAL_GIT" -C "$canonical" rev-parse HEAD)"

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
# Derive the agents' worktrees from the SHARED PROJECT's OWN worktree list. The
# supervisor owns the project repo, so this can't be redirected by an agent
# rewriting its binding.json under $AGENTIC_GIT_HOME (fugu review: HOME is
# agent-writable at same-uid — see the honest boundary in the README).
wt_for_branch() { "$REAL_GIT" -C "$project" worktree list --porcelain 2>/dev/null | awk -v b="refs/heads/$1" '/^worktree /{w=$2} $0==("branch " b){print w; exit}'; }
rbr() { "$REAL_GIT" -C "$1" symbolic-ref --short HEAD 2>/dev/null; }
wa="$(wt_for_branch feat/a)"; wb="$(wt_for_branch feat/b)"

# I1 two distinct worktrees LINKED TO THE SHARED PROJECT (from its own worktree
#    list), each STILL on its own bound branch (real git — catches a cross-branch
#    checkout that drifted HEAD)
if [ -n "$wa" ] && [ -n "$wb" ] && [ "$wa" != "$wb" ]; then
  pass "the shared project has two distinct agent worktrees (from its own worktree list)"
else bad "the shared project lacks two distinct agent worktrees (a=$wa b=$wb)"; fi
if [ "$(rbr "$wa")" = feat/a ] && [ "$(rbr "$wb")" = feat/b ]; then
  pass "each worktree HEAD is on its own bound branch (no cross-branch drift)"
else bad "a worktree drifted off its branch (a=$(rbr "$wa") b=$(rbr "$wb"))"; fi

# I2 origin branches distinct + each trailered to its OWN agent — this alone
#    catches a cross-agent force-push clobber or delete (no agent word trusted)
ta="$("$REAL_GIT" -C "$bare" rev-parse feat/a 2>/dev/null)"; tb="$("$REAL_GIT" -C "$bare" rev-parse feat/b 2>/dev/null)"
if [ -n "$ta" ] && [ -n "$tb" ] && [ "$ta" != "$tb" ]; then
  pass "both branches on the shared origin, distinct tips (neither deleted/collapsed)"
else bad "an origin branch was clobbered or deleted (a=$ta b=$tb)"; fi
if "$REAL_GIT" -C "$bare" log -1 --format=%B feat/a 2>/dev/null | grep -q "Agentic-Agent: agent-a" \
   && "$REAL_GIT" -C "$bare" log -1 --format=%B feat/b 2>/dev/null | grep -q "Agentic-Agent: agent-b"; then
  pass "each origin branch's tip is trailered to its OWN agent (not clobbered)"
else bad "origin provenance is wrong or one branch was clobbered by the other agent"; fi

# I4 the shared source repo AND your stand-in real checkout were NOT moved. The
#    canonical check independently catches an agent that touched your checkout,
#    regardless of what the agent reported (closes fugu's forged-artifact hole).
if [ "$("$REAL_GIT" -C "$project" rev-parse HEAD)" = "$PROJECT_BASE" ] \
   && [ "$("$REAL_GIT" -C "$project" rev-parse --abbrev-ref HEAD)" = main ]; then
  pass "the shared source repo's HEAD is untouched"
else bad "the shared source repo drifted"; fi
if [ "$("$REAL_GIT" -C "$canonical" rev-parse HEAD)" = "$CANON_BASE" ]; then
  pass "your stand-in real checkout's HEAD is untouched (containment held)"
else bad "the canonical checkout HEAD moved ($CANON_BASE -> $("$REAL_GIT" -C "$canonical" rev-parse HEAD))"; fi

# I5 one 32-byte key, both signed bindings (no split-brain)
ks=$(wc -c < "$home/.config-integrity-key" 2>/dev/null | tr -d ' ')
if [ "$ks" = 32 ] && [ -f "$home/runtime/agent-a/binding.json.sig" ] && [ -f "$home/runtime/agent-b/binding.json.sig" ]; then
  pass "one 32-byte integrity key; both agents' signed bindings present"
else bad "key/binding split-brain (keysize=$ks)"; fi

# I6 both agents appear, attributed, in the shared audit log — independent proof
#    each went through the shim and its activity was recorded to it.
elog="$home/fleet_events.jsonl"
if grep -q "agent-a" "$elog" 2>/dev/null && grep -q "agent-b" "$elog" 2>/dev/null; then
  pass "both agents' activity is recorded, per-agent, in the shared audit log"
else bad "audit log missing per-agent attribution"; fi

# I7 CONSISTENCY cross-check: each agent's OWN report must AGREE with the truth
#    above. Authority is the independent state (I1-I6); this only flags an agent
#    whose self-report disagrees — it can NEVER turn a real violation into a pass.
consistent() { role="$1"; ok=0
  [ "$(cat "$arts/$role/verdict.txt" 2>/dev/null)" = PASS ] || ok=1
  for n in $(seq 1 10); do
    exp=$(cat "$arts/$role/$n.expect" 2>/dev/null); rc=$(cat "$arts/$role/$n.rc" 2>/dev/null)
    case "$exp" in ok) [ "$rc" = 0 ] || ok=1 ;; deny) [ "$rc" != 0 ] || ok=1 ;; *) ok=1 ;; esac
  done
  return "$ok"
}
if consistent a; then pass "agent-a's self-report agrees with the re-derived state"; else bad "agent-a self-report inconsistent"; fi
if consistent b; then pass "agent-b's self-report agrees with the re-derived state"; else bad "agent-b self-report inconsistent"; fi

if [ "$FAILS" = 0 ]; then
  say "$(c '1;32')MULTI-AGENT SCENARIO VERIFIED$(c 0) — every invariant held (re-derived from state): two agents shared one repo, stayed isolated, kept per-agent provenance, and could not clobber each other or touch your checkout."
else
  say "$(c '1;31')FAILED$(c 0) — $FAILS invariant(s) did not hold."
fi
[ "$KEEP" = 1 ] && printf '\n  world kept at: %s\n' "$work"
exit "$([ "$FAILS" = 0 ] && echo 0 || echo 1)"
