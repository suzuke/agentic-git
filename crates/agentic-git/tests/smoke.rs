//! End-to-end smoke tests: drive the SHIPPED binary through the real
//! `agentic-git run` session flow (the way a user actually invokes it), not
//! the internal helpers. If these break, the tool is broken for users.
//! Also pins the three PR-#8-review findings (packaging is covered by CI's
//! `cargo package` gate; #2 PATH-separator refusal and #3 marker-clean here).

use std::path::{Path, PathBuf};
use std::process::Command;

fn real_git() -> PathBuf {
    for dir in std::env::split_paths(&std::env::var_os("PATH").unwrap_or_default()) {
        let g = dir.join("git");
        if g.exists() {
            let s = g.to_string_lossy();
            if !s.contains(".agend-terminal") && !s.contains(".agentic-git") {
                return g;
            }
        }
    }
    panic!("no real git on PATH");
}

fn sanitized_path(real_git: &Path) -> std::ffi::OsString {
    let mut dirs: Vec<PathBuf> = real_git.parent().map(|p| vec![p.to_path_buf()]).unwrap_or_default();
    for p in std::env::split_paths(&std::env::var_os("PATH").unwrap_or_default()) {
        let s = p.to_string_lossy();
        if !s.contains(".agend-terminal") && !s.contains(".agentic-git") {
            dirs.push(p);
        }
    }
    std::env::join_paths(dirs).unwrap()
}

fn tmp(tag: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!(
        "agentic-git-smoke-{tag}-{}-{}",
        std::process::id(),
        std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos()
    ));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

fn git(real_git: &Path, args: &[&str], cwd: &Path) {
    let o = Command::new(real_git)
        .args(args).current_dir(cwd)
        .env("AGENTIC_GIT_BYPASS", "1").env("AGEND_GIT_BYPASS", "1")
        .env("GIT_AUTHOR_NAME", "t").env("GIT_AUTHOR_EMAIL", "t@t")
        .env("GIT_COMMITTER_NAME", "t").env("GIT_COMMITTER_EMAIL", "t@t")
        .output().expect("git");
    assert!(o.status.success(), "git {args:?}: {}", String::from_utf8_lossy(&o.stderr));
}

fn init_repo(real_git: &Path, dir: &Path) {
    git(real_git, &["init", "-q", "-b", "main", "."], dir);
    std::fs::write(dir.join("README.md"), "hello\n").unwrap();
    git(real_git, &["add", "."], dir);
    git(real_git, &["commit", "-q", "-m", "init"], dir);
}

/// Invoke `agentic-git run` (argv[0] = the binary name → CLI mode, no override).
fn run_session(repo: &Path, home: &Path, real_git: &Path, agent: &str, branch: &str, script: &str) -> std::process::Output {
    Command::new(env!("CARGO_BIN_EXE_agentic-git"))
        .args(["run", "--agent", agent, "--branch", branch, "--", "sh", "-c", script])
        .current_dir(repo)
        .env("AGENTIC_GIT_HOME", home)
        .env("AGENTIC_GIT_REAL_GIT", real_git)
        .env("PATH", sanitized_path(real_git))
        .env_remove("AGEND_HOME").env_remove("AGEND_INSTANCE_NAME").env_remove("AGEND_REAL_GIT")
        .env_remove("AGENTIC_GIT_BYPASS").env_remove("AGEND_GIT_BYPASS")
        .env_remove("AGENTIC_GIT_SNAPSHOTS").env_remove("AGEND_GIT_SNAPSHOTS")
        .output().expect("agentic-git run")
}

/// THE smoke test: a real session routes to the bound branch, denies a
/// cross-branch checkout, snapshots + recovers a `reset --hard`, and stamps
/// the provenance trailer — all through the shipped binary.
#[test]
fn run_session_end_to_end_smoke() {
    let root = tmp("e2e");
    let repo = root.join("repo");
    std::fs::create_dir_all(&repo).unwrap();
    let rg = real_git();
    init_repo(&rg, &repo);
    let home = root.join("home");

    let script = r#"
set -u
echo "BRANCH=$(git rev-parse --abbrev-ref HEAD)"
echo v1 > tracked.txt; git add tracked.txt; git commit -q -m "smoke add"
git log -1 --format=%B | grep -q "Agentic-Agent:" && echo TRAILER=yes || echo TRAILER=no
echo DIRTY > tracked.txt
git reset --hard >/dev/null 2>&1 && echo RESET=ran
echo "SNAPS=$(git for-each-ref refs/agentic-git/snapshots/ | wc -l | tr -d ' ')"
if git checkout main >/dev/null 2>&1; then echo CHECKOUT_MAIN=allowed; else echo CHECKOUT_MAIN=denied; fi
"#;
    let out = run_session(&repo, &home, &rg, "smoke", "feat/smoke", script);
    let s = String::from_utf8_lossy(&out.stdout);
    assert!(out.status.success(), "run failed: {}\n{}", String::from_utf8_lossy(&out.stderr), s);
    assert!(s.contains("BRANCH=feat/smoke"), "routing to bound branch:\n{s}");
    assert!(s.contains("TRAILER=yes"), "provenance trailer stamped:\n{s}");
    assert!(s.contains("RESET=ran"), "destructive op still executes:\n{s}");
    assert!(s.contains("SNAPS=1"), "reset --hard on a DIRTY tree snapshots once:\n{s}");
    assert!(s.contains("CHECKOUT_MAIN=denied"), "cross-branch checkout denied:\n{s}");
    let _ = std::fs::remove_dir_all(&root);
}

/// Review #3: a fresh session's ONLY untracked entry is `.agend-managed`; a
/// `reset --hard` on that (otherwise clean) tree must be skip-when-clean —
/// zero snapshot refs. Guards against the marker defeating the fast path.
#[test]
fn run_session_marker_only_tree_is_clean_smoke() {
    let root = tmp("marker");
    let repo = root.join("repo");
    std::fs::create_dir_all(&repo).unwrap();
    let rg = real_git();
    init_repo(&rg, &repo);
    let home = root.join("home");

    let script = r#"
# nothing changed except the provisioned .agend-managed marker
git reset --hard >/dev/null 2>&1
echo "SNAPS=$(git for-each-ref refs/agentic-git/snapshots/ | wc -l | tr -d ' ')"
"#;
    let out = run_session(&repo, &home, &rg, "marker", "feat/marker", script);
    let s = String::from_utf8_lossy(&out.stdout);
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    assert!(s.contains("SNAPS=0"), "marker-only tree must be clean → no snapshot:\n{s}");
    let _ = std::fs::remove_dir_all(&root);
}

/// Review #2: an `AGENTIC_GIT_HOME` containing a PATH-list separator must make
/// `run` REFUSE (exit 78) rather than silently launch the agent unguarded.
#[test]
#[cfg(unix)]
fn run_refuses_home_with_path_separator_smoke() {
    let root = tmp("colon");
    let repo = root.join("repo");
    std::fs::create_dir_all(&repo).unwrap();
    let rg = real_git();
    init_repo(&rg, &repo);
    let home = root.join("ho:me"); // ':' — unrepresentable in PATH

    let out = run_session(&repo, &home, &rg, "colon", "feat/colon", "true");
    assert_eq!(
        out.status.code(),
        Some(78),
        "must refuse (EX_CONFIG), not launch unguarded; stderr={}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(
        String::from_utf8_lossy(&out.stderr).contains("guarded PATH"),
        "must explain why: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let _ = std::fs::remove_dir_all(&root);
}
