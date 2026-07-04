//! agentic-git session mode — the CLI surface reached when argv[0] does NOT
//! look like `git` (see `is_git_invocation` in `main.rs`). This absorbs the
//! **minimal** orchestrator role for a standalone user: provisioning a
//! worktree, a signed binding, and hooks, then launching the agent inside the
//! guarded session (see `agentic-git` issue #1 — this module implements that
//! design literally; do not re-derive it from first principles here).
//!
//! Subcommands: `run`, `version`. Anything else — including no subcommand —
//! is a hard `exit(2)` with usage (Δ1): CLI mode never silently falls back to
//! shim behavior.

use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::process::Command;

use agentic_git_core::integrity_core;

/// Disk-contract filename for the shared HMAC key (mirrors
/// `agentic_git_core::integrity_core::KEY_FILE`, which is `pub(crate)` to
/// that crate and therefore not reachable from here — this is the *public*
/// disk contract the INVARIANT clause documents, not a private implementation
/// detail we're reaching into). `sign`/`verify` themselves are the only
/// public API session mode calls.
const KEY_FILE: &str = ".config-integrity-key";
const KEY_LEN: usize = 32;

pub fn cli_main() -> ! {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match args.first().map(String::as_str) {
        Some("run") => run_cmd(&args[1..]),
        Some("version") => {
            println!("agentic-git {} (cli)", env!("CARGO_PKG_VERSION"));
            std::process::exit(0);
        }
        Some(other) => {
            print_usage(Some(other));
            std::process::exit(2);
        }
        None => {
            print_usage(None);
            std::process::exit(2);
        }
    }
}

fn print_usage(bad: Option<&str>) {
    match bad {
        Some(cmd) => eprintln!("agentic-git: unknown subcommand `{cmd}`"),
        None => eprintln!("agentic-git: no subcommand given"),
    }
    eprintln!(
        "Usage:\n  \
         agentic-git run [--agent <name>] [--branch <branch>] [--base <ref>] -- <cmd...>\n  \
         agentic-git version\n\n\
         To use the shim, invoke via <home>/bin/git (see README)."
    );
}

// ── `run` argument parsing (hand-rolled — no clap, dependency discipline) ──

#[derive(Debug, Default, PartialEq, Eq)]
struct RunArgs {
    agent: Option<String>,
    branch: Option<String>,
    base: Option<String>,
    cmd: Vec<String>,
}

fn parse_run_args(args: &[String]) -> Result<RunArgs, String> {
    let mut out = RunArgs::default();
    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "--" => {
                out.cmd = args[i + 1..].to_vec();
                if out.cmd.is_empty() {
                    return Err("`run` requires a command after `--`".to_string());
                }
                return Ok(out);
            }
            "--agent" => {
                i += 1;
                out.agent = Some(
                    args.get(i)
                        .ok_or("`--agent` requires a value")?
                        .to_string(),
                );
            }
            "--branch" => {
                i += 1;
                out.branch = Some(
                    args.get(i)
                        .ok_or("`--branch` requires a value")?
                        .to_string(),
                );
            }
            "--base" => {
                i += 1;
                out.base = Some(args.get(i).ok_or("`--base` requires a value")?.to_string());
            }
            other => return Err(format!("unknown flag `{other}` for `run`")),
        }
        i += 1;
    }
    Err("`run` requires `-- <cmd...>`".to_string())
}

// ── Δ3 v3: `--agent` validation — pure fn, Windows/case-insensitive-safe ──

const RESERVED_WINDOWS_DEVICE_NAMES: &[&str] = &[
    "CON", "PRN", "AUX", "NUL", "COM1", "COM2", "COM3", "COM4", "COM5", "COM6", "COM7", "COM8",
    "COM9", "LPT1", "LPT2", "LPT3", "LPT4", "LPT5", "LPT6", "LPT7", "LPT8", "LPT9",
];

const AGENT_NAME_RULE: &str = "--agent must match ^[a-z0-9][a-z0-9._-]{0,63}$ (lowercase only), \
     must not end with '.', and its stem before the first '.' (uppercased) must not be a \
     reserved Windows device name (CON, PRN, AUX, NUL, COM1-9, LPT1-9)";

/// Δ3 v3 agent-name contract. Pure function — no filesystem access — so it is
/// unit-testable in isolation. `Err` carries the rule text verbatim (the CLI
/// quotes it back to the user on violation).
fn validate_agent_name(name: &str) -> Result<(), &'static str> {
    if name.is_empty() || name.len() > 64 {
        return Err(AGENT_NAME_RULE);
    }
    let bytes = name.as_bytes();
    let is_lower_alnum = |b: u8| b.is_ascii_digit() || b.is_ascii_lowercase();
    let is_allowed = |b: u8| is_lower_alnum(b) || matches!(b, b'.' | b'_' | b'-');
    if !is_lower_alnum(bytes[0]) {
        return Err(AGENT_NAME_RULE);
    }
    if !bytes.iter().all(|&b| is_allowed(b)) {
        return Err(AGENT_NAME_RULE);
    }
    if name.ends_with('.') {
        return Err(AGENT_NAME_RULE);
    }
    let stem = name.split('.').next().unwrap_or(name);
    if RESERVED_WINDOWS_DEVICE_NAMES.contains(&stem.to_ascii_uppercase().as_str()) {
        return Err(AGENT_NAME_RULE);
    }
    Ok(())
}

fn default_agent_name() -> String {
    let mut buf = [0u8; 6];
    match getrandom::fill(&mut buf) {
        Ok(()) => format!("run-{}", hex_lower(&buf)),
        Err(_) => {
            // Extremely unlikely (OS RNG unavailable); fall back to a
            // monotonic-ish, still lowercase-alnum, still-valid identity
            // rather than hard-failing session start over cosmetic naming.
            let nanos = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or(0);
            format!("run-{:x}", nanos as u64)
        }
    }
}

fn hex_lower(bytes: &[u8]) -> String {
    let mut s = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        s.push_str(&format!("{b:02x}"));
    }
    s
}

fn default_branch(agent: &str) -> String {
    format!(
        "agent/{agent}/{}",
        chrono::Utc::now().format("%Y%m%d-%H%M")
    )
}

// ── Home provisioning ───────────────────────────────────────────────────

fn resolve_home() -> PathBuf {
    if let Ok(h) = super::env_compat("AGENTIC_GIT_HOME") {
        if !h.is_empty() {
            return PathBuf::from(h);
        }
    }
    let home_dir = std::env::var_os("HOME")
        .or_else(|| std::env::var_os("USERPROFILE"))
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("."));
    home_dir.join(".agentic-git")
}

/// Provision `bin/`, `runtime/`, `worktrees/`, `hooks/`, and the integrity
/// key (iff missing). A key that can't be written is a hard error — a
/// guarded session without a signable binding must not silently degrade.
fn provision_home(home: &Path) -> Result<(), String> {
    for d in ["bin", "runtime", "worktrees", "hooks"] {
        let p = home.join(d);
        std::fs::create_dir_all(&p).map_err(|e| format!("create {}: {e}", p.display()))?;
    }
    ensure_key(home)
}

/// Δ2: atomic, lock-free first-writer-wins key provisioning. Write to a
/// unique temp file, fsync, then `hard_link` it into place — `AlreadyExists`
/// means we lost the race, not that we failed; we discard our tmp and defer
/// to the survivor. The key path only ever appears fully written (never a
/// partial/truncated file), matching `integrity_core::read_key`'s
/// exactly-32-bytes contract.
fn ensure_key(home: &Path) -> Result<(), String> {
    let key_path = home.join(KEY_FILE);
    if let Ok(meta) = std::fs::metadata(&key_path) {
        if meta.len() as usize == KEY_LEN {
            return Ok(()); // already provisioned — reuse.
        }
        return Err(format!(
            "integrity key at {} exists but is not exactly {KEY_LEN} bytes (corrupt) — refusing \
             to overwrite; a guarded session without a signable binding must not silently \
             degrade. Remove it manually only if you are certain it is safe to regenerate.",
            key_path.display()
        ));
    }

    let mut rand_suffix = [0u8; 8];
    getrandom::fill(&mut rand_suffix).map_err(|e| format!("getrandom: {e}"))?;
    let tmp_path = home.join(format!(
        "key.tmp.{}.{}",
        std::process::id(),
        hex_lower(&rand_suffix)
    ));

    let mut key = [0u8; KEY_LEN];
    getrandom::fill(&mut key).map_err(|e| format!("getrandom: {e}"))?;
    std::fs::write(&tmp_path, key).map_err(|e| format!("write temp key: {e}"))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&tmp_path, std::fs::Permissions::from_mode(0o600))
            .map_err(|e| format!("chmod temp key: {e}"))?;
    }
    // fsync before the hard_link "publish" — the link must never observe a
    // not-yet-durable write.
    if let Ok(f) = std::fs::File::open(&tmp_path) {
        let _ = f.sync_all();
    }

    match std::fs::hard_link(&tmp_path, &key_path) {
        Ok(()) => {
            let _ = std::fs::remove_file(&tmp_path);
            Ok(())
        }
        Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {
            // Someone else won the race; their key stands.
            let _ = std::fs::remove_file(&tmp_path);
            Ok(())
        }
        Err(e) => {
            let _ = std::fs::remove_file(&tmp_path);
            Err(format!("hard_link key provisioning: {e}"))
        }
    }
}

// ── Preconditions (step 1) ─────────────────────────────────────────────

fn git_bypass_cmd(git: &str) -> Command {
    let mut cmd = Command::new(git);
    cmd.env("AGENTIC_GIT_BYPASS", "1").env("AGEND_GIT_BYPASS", "1");
    cmd
}

fn resolve_source_repo(git: &str) -> Result<PathBuf, String> {
    let out = git_bypass_cmd(git)
        .args(["rev-parse", "--show-toplevel"])
        .output()
        .map_err(|e| format!("failed to run git: {e}"))?;
    if !out.status.success() {
        return Err(
            "cwd is not inside a git repository — `run` requires a non-bare git repo".to_string(),
        );
    }
    let top = String::from_utf8_lossy(&out.stdout).trim().to_string();
    if top.is_empty() {
        return Err("git rev-parse --show-toplevel returned no path".to_string());
    }

    let bare_out = git_bypass_cmd(git)
        .args(["rev-parse", "--is-bare-repository"])
        .output()
        .map_err(|e| format!("failed to run git: {e}"))?;
    let is_bare = String::from_utf8_lossy(&bare_out.stdout).trim() == "true";
    if is_bare {
        return Err("cwd is inside a bare git repository — `run` requires a non-bare repo".into());
    }
    Ok(PathBuf::from(top))
}

// ── Δ4: binding reuse predicate ─────────────────────────────────────────

fn canonical_or_lexical(p: &Path) -> PathBuf {
    std::fs::canonicalize(p).unwrap_or_else(|_| p.to_path_buf())
}

/// `git rev-parse --git-common-dir` run inside `dir`, canonicalized. Used to
/// prove a worktree actually belongs to the repo we think it does (Δ4).
fn git_common_dir(git: &str, dir: &Path) -> Option<PathBuf> {
    let out = git_bypass_cmd(git)
        .args(["rev-parse", "--git-common-dir"])
        .current_dir(dir)
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    let raw = String::from_utf8_lossy(&out.stdout).trim().to_string();
    if raw.is_empty() {
        return None;
    }
    let p = PathBuf::from(&raw);
    let abs = if p.is_absolute() { p } else { dir.join(p) };
    Some(canonical_or_lexical(&abs))
}

/// `None` → no existing binding, proceed with fresh provisioning.
/// `Some(Ok(()))` → reuse the existing worktree/binding as-is.
/// `Some(Err(reason))` → hard error; caller must NOT provision anything.
#[allow(clippy::too_many_arguments)]
fn check_reuse(
    home: &Path,
    agent: &str,
    requested_branch: &str,
    requested_worktree: &Path,
    source_repo: &Path,
    git: &str,
) -> Option<Result<(), String>> {
    let dir = home.join("runtime").join(agent);
    let content = std::fs::read_to_string(dir.join("binding.json")).ok()?;
    let existing: serde_json::Value = serde_json::from_str(&content).ok()?;

    let remedy = format!(
        "stale binding at {} — remove it, and if the worktree still exists remove it too \
         (`git worktree remove <path>`), then re-run",
        dir.display()
    );
    let eb_branch = existing["branch"].as_str().unwrap_or_default();
    let eb_worktree = existing["worktree"].as_str().unwrap_or_default();
    let eb_source = existing["source_repo"].as_str().unwrap_or_default();

    if eb_branch != requested_branch {
        return Some(Err(format!(
            "agent {agent:?} is already bound to branch {eb_branch:?}, which does not match the \
             requested branch {requested_branch:?}; {remedy}"
        )));
    }
    let requested_worktree_str = requested_worktree.to_string_lossy();
    if eb_worktree != requested_worktree_str {
        return Some(Err(format!(
            "agent {agent:?} is already bound to worktree {eb_worktree:?}, which does not match \
             the requested worktree {requested_worktree_str:?}; {remedy}"
        )));
    }
    if canonical_or_lexical(Path::new(eb_source)) != canonical_or_lexical(source_repo) {
        return Some(Err(format!(
            "agent {agent:?}'s binding names a different source_repo ({eb_source:?}) than the \
             current repo ({}); {remedy}",
            source_repo.display()
        )));
    }
    if !requested_worktree.exists() {
        return Some(Err(format!(
            "agent {agent:?}'s bound worktree no longer exists on disk ({requested_worktree_str}); \
             {remedy}"
        )));
    }
    let wt_common = git_common_dir(git, requested_worktree);
    let repo_common = git_common_dir(git, source_repo);
    match (wt_common, repo_common) {
        (Some(a), Some(b)) if a == b => Some(Ok(())),
        _ => Some(Err(format!(
            "agent {agent:?}'s bound worktree does not resolve back into this repo's gitdir \
             (ownership check failed); {remedy}"
        ))),
    }
}

// ── Hooks (step 6) ───────────────────────────────────────────────────────

const HOOK_PREPARE_COMMIT_MSG: &str = include_str!("../../../assets/hooks/prepare-commit-msg");
const HOOK_REFERENCE_TRANSACTION: &str = include_str!("../../../assets/hooks/reference-transaction");
#[cfg(windows)]
const HOOK_PREPARE_COMMIT_MSG_PS1: &str =
    include_str!("../../../assets/hooks/prepare-commit-msg.ps1");

fn write_hook(path: &Path, content: &str) -> Result<(), String> {
    std::fs::write(path, content).map_err(|e| format!("write hook {}: {e}", path.display()))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755))
            .map_err(|e| format!("chmod hook {}: {e}", path.display()))?;
    }
    Ok(())
}

fn install_hooks(home: &Path) -> Result<(), String> {
    let hooks_dir = home.join("hooks");
    std::fs::create_dir_all(&hooks_dir).map_err(|e| format!("mkdir hooks: {e}"))?;
    write_hook(
        &hooks_dir.join("prepare-commit-msg"),
        HOOK_PREPARE_COMMIT_MSG,
    )?;
    write_hook(
        &hooks_dir.join("reference-transaction"),
        HOOK_REFERENCE_TRANSACTION,
    )?;
    #[cfg(windows)]
    write_hook(
        &hooks_dir.join("prepare-commit-msg.ps1"),
        HOOK_PREPARE_COMMIT_MSG_PS1,
    )?;
    Ok(())
}

/// Amended step 6: `extensions.worktreeConfig` is a repo-wide switch (must
/// land in the SHARED config), `core.hooksPath` is set `--worktree`-scoped so
/// only THIS worktree gets it — the user's own checkout keeps its hooks
/// (Δ5 hook noninterference).
///
/// `extensions.worktreeConfig` writes the SHARED `$GIT_DIR/config` (it isn't
/// per-worktree until it's set), so two `run`s racing against the SAME source
/// repo (different agents/branches, same first-use) can collide on git's own
/// `config.lock`. That's a plain transient lock, not a correctness issue —
/// retry briefly instead of hard-failing session start over it.
fn configure_worktree_hooks(git: &str, worktree: &Path, hooks_dir: &Path) -> Result<(), String> {
    let run = |args: &[&str]| -> Result<(), String> {
        let mut last_err = String::new();
        for attempt in 0..20 {
            if attempt > 0 {
                std::thread::sleep(std::time::Duration::from_millis(50));
            }
            let out = git_bypass_cmd(git)
                .args(args)
                .current_dir(worktree)
                .output()
                .map_err(|e| e.to_string())?;
            if out.status.success() {
                return Ok(());
            }
            last_err = String::from_utf8_lossy(&out.stderr).trim().to_string();
            if !last_err.contains("could not lock config file") {
                break;
            }
        }
        Err(format!("git {} failed: {last_err}", args.join(" ")))
    };
    run(&["config", "extensions.worktreeConfig", "true"])?;
    run(&[
        "config",
        "--worktree",
        "core.hooksPath",
        &hooks_dir.to_string_lossy(),
    ])?;
    Ok(())
}

// ── Shim wiring (step 7) ────────────────────────────────────────────────

fn ensure_shim_symlink(home: &Path) -> Result<(), String> {
    let bin_dir = home.join("bin");
    std::fs::create_dir_all(&bin_dir).map_err(|e| format!("mkdir bin: {e}"))?;
    let target_name = if cfg!(windows) { "git.exe" } else { "git" };
    let bin_git = bin_dir.join(target_name);
    let exe = std::env::current_exe().map_err(|e| format!("current_exe: {e}"))?;

    #[cfg(unix)]
    {
        if let Ok(existing) = std::fs::read_link(&bin_git) {
            if existing == exe {
                return Ok(()); // already correct — idempotent.
            }
        }
        let _ = std::fs::remove_file(&bin_git);
        std::os::unix::fs::symlink(&exe, &bin_git)
            .map_err(|e| format!("symlink {}: {e}", bin_git.display()))?;
    }
    #[cfg(not(unix))]
    {
        // Windows: copy, not symlink (symlinks need elevated privileges by
        // default). Re-copy unconditionally — cheap, and always correct.
        std::fs::copy(&exe, &bin_git).map_err(|e| format!("copy to {}: {e}", bin_git.display()))?;
    }
    Ok(())
}

// ── Signal passthrough (best-effort; step 9 "signals forwarded") ───────

/// The terminal delivers SIGINT/SIGTERM to the whole foreground process
/// group already (parent AND child, since we don't put the child in its own
/// group) — so the child sees the signal without us doing anything. What we
/// DO need is for the PARENT to survive it long enough to `wait()` the child
/// and print the summary, instead of dying immediately under the default
/// disposition. A real (non-`SIG_IGN`) handler achieves that: it interrupts
/// the blocking `waitpid` with `EINTR` (which `Child::wait` retries) without
/// terminating us, and — unlike `SIG_IGN` — does NOT propagate across the
/// child's `exec` (so the child keeps normal default signal behavior).
#[cfg(unix)]
fn install_signal_passthrough() {
    extern "C" fn noop_handler(_: i32) {}
    unsafe {
        libc::signal(libc::SIGINT, noop_handler as *const () as usize);
        libc::signal(libc::SIGTERM, noop_handler as *const () as usize);
    }
}
#[cfg(not(unix))]
fn install_signal_passthrough() {
    // v1 known limitation: Windows Ctrl+C handling is left to the platform
    // default (the parent may exit before printing the summary).
}

// ── Spawn (step 8) ───────────────────────────────────────────────────────

fn spawn_agent(cmd: &[String], worktree: &Path, home: &Path, agent: &str, real_git: &str) -> i32 {
    let bin_dir = home.join("bin");
    let path_env = std::env::var_os("PATH").unwrap_or_default();
    let new_path_parts: Vec<PathBuf> = std::iter::once(bin_dir)
        .chain(std::env::split_paths(&path_env))
        .collect();
    let new_path: OsString =
        std::env::join_paths(new_path_parts).unwrap_or(path_env);

    let (prog, rest) = match cmd.split_first() {
        Some(pair) => pair,
        None => {
            eprintln!("agentic-git: run: internal error — empty command");
            return 2;
        }
    };

    install_signal_passthrough();

    let mut child = Command::new(prog);
    child
        .args(rest)
        .current_dir(worktree)
        .env("PATH", new_path)
        .env("AGENTIC_GIT_HOME", home)
        .env("AGENTIC_GIT_AGENT", agent)
        .env("AGENTIC_GIT_REAL_GIT", real_git);

    match child.status() {
        Ok(status) => {
            #[cfg(unix)]
            {
                use std::os::unix::process::ExitStatusExt;
                if let Some(sig) = status.signal() {
                    return 128 + sig;
                }
            }
            status.code().unwrap_or(1)
        }
        Err(e) => {
            eprintln!("agentic-git: run: failed to spawn agent command {prog:?}: {e}");
            127
        }
    }
}

// ── `run` orchestration ──────────────────────────────────────────────────

fn run_cmd(raw_args: &[String]) -> ! {
    let parsed = match parse_run_args(raw_args) {
        Ok(p) => p,
        Err(e) => {
            eprintln!("agentic-git: run: {e}");
            print_usage(Some("run"));
            std::process::exit(2);
        }
    };

    let real_git = super::resolve_real_git();

    // Step 1: preconditions.
    let source_repo = match resolve_source_repo(&real_git) {
        Ok(p) => p,
        Err(e) => {
            eprintln!("agentic-git: run: {e}");
            std::process::exit(1);
        }
    };

    // Step 2: home.
    let home = resolve_home();
    if let Err(e) = provision_home(&home) {
        eprintln!("agentic-git: run: {e}");
        std::process::exit(1);
    }

    // Step 3: identity.
    let agent = parsed.agent.clone().unwrap_or_else(default_agent_name);
    if let Err(rule) = validate_agent_name(&agent) {
        eprintln!("agentic-git: run: invalid --agent {agent:?}: {rule}");
        std::process::exit(2);
    }

    // Step 4: worktree (branch/base defaults + Δ4 reuse-or-fresh decision).
    let branch = parsed.branch.clone().unwrap_or_else(|| default_branch(&agent));
    let base = parsed.base.clone().unwrap_or_else(|| "HEAD".to_string());
    let wt_path = home.join("worktrees").join(&agent).join(&branch);

    let reused = match check_reuse(&home, &agent, &branch, &wt_path, &source_repo, &real_git) {
        Some(Ok(())) => true,
        Some(Err(reason)) => {
            eprintln!("agentic-git: run: {reason}");
            std::process::exit(1);
        }
        None => false,
    };

    let issued_at = chrono::Utc::now().to_rfc3339();

    if !reused {
        if let Some(parent) = wt_path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        let wt_path_str = wt_path.to_string_lossy().into_owned();
        let wt_out = git_bypass_cmd(&real_git)
            .args(["worktree", "add", &wt_path_str, "-b", &branch, &base])
            .current_dir(&source_repo)
            .output();
        match wt_out {
            Ok(out) if out.status.success() => {}
            Ok(out) => {
                // Surface git's own refusal verbatim (e.g. a second `run` on
                // the same branch — natural mutual exclusion, no lease
                // machinery in v1).
                eprint!("{}", String::from_utf8_lossy(&out.stderr));
                std::process::exit(out.status.code().unwrap_or(1));
            }
            Err(e) => {
                eprintln!("agentic-git: run: failed to spawn git worktree add: {e}");
                std::process::exit(1);
            }
        }

        let _ = std::fs::write(
            wt_path.join(".agend-managed"),
            format!("agent={agent}\nleased_at={issued_at}\n"),
        );

        // Step 5: binding (schema v1) + HMAC sidecar.
        let binding = serde_json::json!({
            "version": 1,
            "agent": agent,
            "task_id": format!(
                "run-session-{}",
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .map(|d| d.as_secs())
                    .unwrap_or(0)
            ),
            "branch": branch,
            "issued_at": issued_at,
            "worktree": wt_path.to_string_lossy(),
            "source_repo": source_repo.to_string_lossy(),
        });
        let content = match serde_json::to_string_pretty(&binding) {
            Ok(c) => c,
            Err(e) => {
                eprintln!("agentic-git: run: serialize binding: {e}");
                std::process::exit(1);
            }
        };
        let runtime_dir = home.join("runtime").join(&agent);
        if let Err(e) = std::fs::create_dir_all(&runtime_dir) {
            eprintln!("agentic-git: run: mkdir {}: {e}", runtime_dir.display());
            std::process::exit(1);
        }
        if let Err(e) = std::fs::write(runtime_dir.join("binding.json"), &content) {
            eprintln!("agentic-git: run: write binding.json: {e}");
            std::process::exit(1);
        }
        let sig = integrity_core::sign(&home, content.as_bytes());
        if let Err(e) = std::fs::write(runtime_dir.join("binding.json.sig"), sig) {
            eprintln!("agentic-git: run: write binding.json.sig: {e}");
            std::process::exit(1);
        }
    }

    // Step 6: hooks — embed + wire, always (idempotent whether fresh or reused).
    if let Err(e) = install_hooks(&home) {
        eprintln!("agentic-git: run: {e}");
        std::process::exit(1);
    }
    if let Err(e) = configure_worktree_hooks(&real_git, &wt_path, &home.join("hooks")) {
        eprintln!("agentic-git: run: {e}");
        std::process::exit(1);
    }

    // Step 7: shim wiring.
    if let Err(e) = ensure_shim_symlink(&home) {
        eprintln!("agentic-git: run: {e}");
        std::process::exit(1);
    }

    // Step 8: spawn.
    let code = spawn_agent(&parsed.cmd, &wt_path, &home, &agent, &real_git);

    // Step 9: teardown = keep. Print the session summary regardless of the
    // child's exit code — the work must outlive the agent.
    eprintln!(
        "\nagentic-git: session ended (exit {code}).\n  \
         worktree: {}\n  \
         branch:   {branch}\n  \
         re-enter: cd {} && git status\n  \
         remove:   git -C {} worktree remove {} && rm -rf {}",
        wt_path.display(),
        wt_path.display(),
        source_repo.display(),
        wt_path.display(),
        home.join("runtime").join(&agent).display(),
    );
    std::process::exit(code);
}

#[cfg(test)]
mod tests;
