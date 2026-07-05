# demo

## `recovery-demo.sh` — an agent erased my work; one command put it back

The 60-second story the tool exists for, as a **real guarded session** you can
run or screen-record:

1. A throwaway project with one committed file.
2. An agent is launched in a guarded session with a single `agentic-git run`.
3. The agent does valuable but **uncommitted** work — then makes a machine-speed
   mistake (`git reset --hard`) that erases it. In plain git it is simply gone.
4. `agentic-git snapshots restore` brings it back in **one command** — no
   snapshot ref, no git internals — landing the recovery *unstaged*.

```sh
./demo/recovery-demo.sh
# or test an installed binary instead of building from source:
AGENTIC_GIT_BIN=$(command -v agentic-git) ./demo/recovery-demo.sh
```

**It doubles as a cold-start acceptance check.** Every step is asserted; it
exits non-zero with a clear `FAIL` if the shipped binary does not actually
recover the work on your machine, and prints `DEMO PASSED` when it does — so a
first-time user can confirm the tool delivers on its promise without reading
past the top of the README.

The framing while you watch: a worktree isolates *where* damage can happen;
agentic-git records *what was about to be lost*, so a destructive op is
undoable — which plain git cannot offer.

### Recording it

```sh
# needs `asciinema` (not bundled)
asciinema rec -c './demo/recovery-demo.sh' recovery.cast
```
