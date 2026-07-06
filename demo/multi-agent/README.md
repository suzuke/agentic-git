# multi-agent verification scenario

Two agents share **one repo, at the same time**, each on its own branch — the
setup agentic-git exists for. This scenario runs it for real and checks the
multi-agent invariants hold.

The design point (from an adversarial review): **an agent's own PASS is not
proof.** Real agents only *execute* and leave a machine-recomputable evidence
bundle; a separate deterministic **supervisor re-derives the truth from git /
home / audit state** and judges. An agent that reports PASS but whose state
disagrees still FAILS.

```sh
./demo/multi-agent/verify.sh          # run it, print the synthesis
./demo/multi-agent/verify.sh --keep   # keep the throwaway world to inspect
```

Uses your installed `agentic-git` (or the repo's release build). Everything is a
throwaway world in a temp dir; the only "remote" is a local bare repo.

## What happens

`verify.sh` (the supervisor) builds a shared project (with a local `origin`) and
a stand-in for *your* real checkout, then launches **agent-a and agent-b
concurrently**, each in its own guarded session (`agentic-git run --agent … --branch …`).
Each runs [`agent-run.sh`](agent-run.sh): it does real work, pushes its own
branch, tries to interfere with the other agent, and saves every command's raw
`stdout/stderr/exit-code` plus an evidence snapshot into its artifact dir.

Then the supervisor **re-derives the truth from state** and checks nine invariants:

1. two distinct worktrees, each on its own bound branch;
2. `git` inside both sessions resolved to the shim (they were genuinely guarded);
3. both branches reached the shared origin with distinct tips;
4. provenance is per-agent and not mixed (feat/a's tip trailers agent-a, feat/b's agent-b);
5. the working trees are isolated — neither agent's file leaked into the other's;
6. the shared source repo's HEAD was never touched;
7. one 32-byte integrity key, both agents' signed bindings present (no split-brain);
8. **every guarded step behaved** — each agent's own push succeeded, and its
   cross-agent ops (checkout / force-push / delete of the other's branch, and
   touching the stand-in real checkout) were all **denied**;
9. each agent's self-verdict is PASS **and** consistent with the re-derived state.

## Why it's a real test, not a rosy demo

Invariant 8 reads the actual exit codes. If a guard fails to fire — e.g. before
the cross-branch push guard landed, `git push origin +HEAD:feat/b` *succeeded*
and clobbered the other agent's branch — the deny step's exit code is `0`, the
synthesis fails, and the scenario reports FAILED. It is designed to catch a
regression of exactly the cross-agent-clobber gap this scenario's design first
surfaced.

## What the supervisor trusts (and what it can't)

The supervisor re-derives every invariant from state **it owns** — the shared
project repo (and its own worktree list), the bare origin, the stand-in
checkout, their recorded base commits. It does **not** trust anything an agent
writes under `$AGENTIC_GIT_HOME` (bindings, evidence files, its own verdict):
those are cross-checked for consistency but can never turn a real violation into
a pass.

**Honest boundary.** agentic-git is a **seatbelt, not a cage** — a same-uid
userspace shim. This scenario proves the guards hold for agents going *through
the shim* (the tool's threat model: accident-prone, semi-trusted agents). It
does **not** — and cannot — defend against a *malicious* same-uid agent that
forges its own `$AGENTIC_GIT_HOME` state (it can even read the HMAC key and
re-sign) or bypasses the shim by calling `/usr/bin/git` directly. For that you
want kernel isolation (containers, `sandbox-exec`, Landlock) *underneath* the
shim. That's why the supervisor keys its checks off repos it owns, not off the
agent's home.

## Two real agent sessions

The scenario runs two real agent *sessions*. Driving them with two real *LLM*
fleet agents (each executing `agent-run.sh` itself in its own shell) uses the
same evidence bundle + the same independent synthesis — the supervisor is still
the verifier.
