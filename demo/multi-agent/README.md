# multi-agent verification scenario

Two agents share **one repo, at the same time**, each on its own branch — the
setup agentic-git exists for. This scenario runs it for real and checks the
multi-agent invariants hold.

The design point (from an adversarial review): **an agent's own PASS is not
proof.** Real agents only *execute* and leave a machine-recomputable evidence
bundle; a separate deterministic **supervisor re-derives the truth from state it
owns** (the shared repos and the audit log — never the agent's home) and judges.
An agent that reports PASS but whose state disagrees still FAILS.

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

Then the supervisor **re-derives the truth from state it owns** — the shared
project's own worktree list, the bare origin, the stand-in checkout, and their
recorded base commits — and checks:

1. the shared project has **two distinct agent worktrees** (from its own worktree
   list), each still **on its own bound branch** (no cross-branch drift);
2. both branches reached the shared origin with **distinct tips**, each tip
   **trailered to its own agent** — so neither branch was clobbered or deleted;
3. the shared source repo's HEAD **and** your stand-in real checkout's HEAD were
   **never moved** (containment held);
4. one 32-byte integrity key with both agents' signed bindings (the concurrent
   provisioning did not split-brain);
5. both agents' activity is recorded, **attributed per-agent**, in the shared
   audit log;
6. each agent's own self-report is **consistent** with the re-derived state — a
   cross-check that can flag a disagreeing report but can never turn a real
   violation into a pass.

## Why it's a real test, not a rosy demo

The load-bearing proofs read **state the supervisor owns**, so a fired guard is
proven by its *consequence*, not by the agent's word. Before the cross-branch
push guard landed, `git push origin +HEAD:feat/b` *succeeded* and clobbered the
other agent's branch — which shows up in the origin as feat/b's tip trailered to
the **wrong** agent (invariant 2), so the synthesis reports FAILED. A touch of
your checkout likewise moves its HEAD (invariant 3). The agents' own exit codes
and verdicts are only the *consistency* cross-check (invariant 6): they can never
make a real violation pass, because every violation is read from owned state, not
from the agent's files.

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
