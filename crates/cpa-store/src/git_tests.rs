//! Go internal/store/gitstore_test.go cases (M4-0361) against local bare remotes. Each
//! test names the Go test it mirrors; the expected values are Go's assertions.
//!
//! Not ported: the go-git handle cases (`TestRecoverRepositoryCloseFailures*`,
//! `TestInspectRecoveryBaselineFailureClosesRepositoryHandle`, rename-failure
//! injection), which test go-git repository handles the git executable does not have,
//! and `TestGitTokenStoreDisabledLoginReachesTokenStorage`, whose `Save` policy lives in
//! the server's persist hooks here, not in the store.

use std::path::{Path, PathBuf};
use std::process::Command;

use super::GitStore;

fn scratch(name: &str) -> PathBuf {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let dir = std::env::temp_dir().join(format!("cpa-gitgo-{name}-{}-{nanos}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

fn git(dir: &Path, args: &[&str]) -> String {
    let output = Command::new("git")
        .current_dir(dir)
        .args(["-c", "user.name=CLIProxyAPI", "-c", "user.email=cliproxy@local"])
        .args(["-c", "commit.gpgsign=false", "-c", "init.defaultBranch=master"])
        .args(args)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "git {args:?}: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8_lossy(&output.stdout).trim().to_owned()
}

/// Go `setupGitRemoteRepository`: a bare remote whose branches each hold `branch.txt`,
/// seeded from `<root>/seed` (later advanced from there), HEAD naming `default`.
fn setup_remote(root: &Path, default: &str, branches: &[(&str, &str)]) -> String {
    let remote = root.join("remote.git");
    std::fs::create_dir_all(&remote).unwrap();
    git(&remote, &["init", "-q", "--bare"]);
    let seed = root.join("seed");
    std::fs::create_dir_all(&seed).unwrap();
    git(&seed, &["init", "-q"]);
    git(&seed, &["symbolic-ref", "HEAD", &format!("refs/heads/{default}")]);
    let marker = |name: &str, contents: &str, message: &str| {
        let _ = name;
        std::fs::write(seed.join("branch.txt"), contents).unwrap();
        git(&seed, &["add", "branch.txt"]);
        git(&seed, &["commit", "-q", "-m", message]);
    };
    let (_, contents) = branches.iter().find(|(name, _)| *name == default).unwrap();
    marker(default, contents, "seed default branch");
    for (name, contents) in branches.iter().filter(|(name, _)| *name != default) {
        git(&seed, &["checkout", "-q", default]);
        git(&seed, &["checkout", "-q", "-b", name]);
        marker(name, contents, &format!("seed branch {name}"));
    }
    git(&seed, &["remote", "add", "origin", &remote.to_string_lossy()]);
    git(&seed, &["push", "-q", "origin", "refs/heads/*:refs/heads/*"]);
    git(&remote, &["symbolic-ref", "HEAD", &format!("refs/heads/{default}")]);
    remote.to_string_lossy().into_owned()
}

/// Go `advanceRemoteBranch` (from the branch) and `advanceRemoteBranchFromNewBranch`
/// (a new branch from master).
fn advance(root: &Path, branch: &str, contents: &str, new_from_master: bool) {
    let seed = root.join("seed");
    if new_from_master {
        git(&seed, &["checkout", "-q", "master"]);
        git(&seed, &["checkout", "-q", "-b", branch]);
    } else {
        git(&seed, &["checkout", "-q", branch]);
    }
    std::fs::write(seed.join("branch.txt"), contents).unwrap();
    git(&seed, &["add", "branch.txt"]);
    git(&seed, &["commit", "-q", "-m", &format!("advance {branch}")]);
    git(
        &seed,
        &[
            "push",
            "-q",
            "origin",
            &format!("refs/heads/{branch}:refs/heads/{branch}"),
        ],
    );
}

fn head_branch(repo: &Path) -> String {
    git(repo, &["symbolic-ref", "--short", "HEAD"])
}

fn read(path: &Path) -> String {
    std::fs::read_to_string(path).unwrap()
}

fn remote_file(remote: &str, branch: &str, path: &str) -> Option<String> {
    let output = Command::new("git")
        .current_dir(remote)
        .args(["show", &format!("{branch}:{path}")])
        .output()
        .unwrap();
    output
        .status
        .success()
        .then(|| String::from_utf8_lossy(&output.stdout).into_owned())
}

fn remote_head(remote: &str) -> String {
    git(Path::new(remote), &["symbolic-ref", "--short", "HEAD"])
}

/// Go `assertRepositoryBranchAndContents`.
fn branch_and_contents(repo: &Path, branch: &str, contents: &str) {
    assert_eq!(head_branch(repo), branch);
    assert_eq!(read(&repo.join("branch.txt")), contents);
}

fn store(remote: &str, branch: &str, repo: &Path) -> GitStore {
    GitStore::new(remote, "", "", branch, repo)
}

/// Writes `auths/<name>` and persists it (Go `Save`).
fn save(store: &GitStore, name: &str, contents: &str) -> PathBuf {
    let path = store.auth_dir().join(name);
    std::fs::write(&path, contents).unwrap();
    store
        .persist_auth_files(&format!("Sync auth {name}"), std::slice::from_ref(&path))
        .unwrap();
    path
}

fn commit(store: &GitStore, message: &str, paths: &[&str]) -> anyhow::Result<()> {
    let mut last_gc = store.lock.lock().unwrap();
    let paths: Vec<String> = paths.iter().map(|p| (*p).to_owned()).collect();
    store.commit_and_push(&mut last_gc, message, &paths, false)
}

#[test]
fn uses_remote_default_branch_when_branch_not_configured() {
    let root = scratch("default");
    let remote = setup_remote(
        &root,
        "trunk",
        &[
            ("trunk", "remote default branch\n"),
            ("release/2026", "release branch\n"),
        ],
    );
    let ws = root.join("workspace");
    let s = store(&remote, "", &ws);
    s.ensure_repository().unwrap();
    branch_and_contents(&ws, "trunk", "remote default branch\n");
    advance(&root, "trunk", "remote default branch updated\n", false);
    advance(&root, "release/2026", "release branch updated\n", false);
    s.ensure_repository().unwrap();
    branch_and_contents(&ws, "trunk", "remote default branch updated\n");
    assert_eq!(remote_head(&remote), "trunk");
}

#[test]
fn uses_configured_branch_when_explicitly_set() {
    let root = scratch("configured");
    let remote = setup_remote(
        &root,
        "trunk",
        &[
            ("trunk", "remote default branch\n"),
            ("release/2026", "release branch\n"),
        ],
    );
    let ws = root.join("workspace");
    let s = store(&remote, "release/2026", &ws);
    s.ensure_repository().unwrap();
    branch_and_contents(&ws, "release/2026", "release branch\n");
    advance(&root, "trunk", "remote default branch updated\n", false);
    advance(&root, "release/2026", "release branch updated\n", false);
    s.ensure_repository().unwrap();
    branch_and_contents(&ws, "release/2026", "release branch updated\n");
    assert_eq!(remote_head(&remote), "trunk");
}

#[test]
fn a_missing_configured_branch_fails() {
    let root = scratch("missing");
    let remote = setup_remote(&root, "trunk", &[("trunk", "remote default branch\n")]);
    assert!(
        store(&remote, "missing-branch", &root.join("workspace"))
            .ensure_repository()
            .is_err()
    );
    assert_eq!(remote_head(&remote), "trunk");

    // Go `...OnExistingRepositoryPull`: the reopened store fails and HEAD stays.
    let root = scratch("missing-pull");
    let remote = setup_remote(&root, "trunk", &[("trunk", "remote default branch\n")]);
    let ws = root.join("workspace");
    store(&remote, "", &ws).ensure_repository().unwrap();
    assert!(store(&remote, "missing-branch", &ws).ensure_repository().is_err());
    assert_eq!(head_branch(&ws), "trunk");
    assert_eq!(remote_head(&remote), "trunk");
}

#[test]
fn initializes_an_empty_remote_using_the_configured_branch() {
    let root = scratch("empty-branch");
    let remote = root.join("remote.git");
    std::fs::create_dir_all(&remote).unwrap();
    git(&remote, &["init", "-q", "--bare"]);
    let remote = remote.to_string_lossy().into_owned();
    let ws = root.join("workspace");
    store(&remote, "feature/gemini-fix", &ws).ensure_repository().unwrap();
    assert_eq!(head_branch(&ws), "feature/gemini-fix");
    git(
        Path::new(&remote),
        &["rev-parse", "--verify", "refs/heads/feature/gemini-fix"],
    );
    let master = Command::new("git")
        .current_dir(&remote)
        .args(["rev-parse", "--verify", "-q", "refs/heads/master"])
        .output()
        .unwrap();
    assert!(!master.status.success(), "no master branch is created");
}

#[test]
fn an_existing_repository_switches_to_the_configured_branch() {
    let root = scratch("switch");
    let remote = setup_remote(
        &root,
        "master",
        &[
            ("master", "remote master branch\n"),
            ("develop", "remote develop branch\n"),
        ],
    );
    let ws = root.join("workspace");
    store(&remote, "", &ws).ensure_repository().unwrap();
    branch_and_contents(&ws, "master", "remote master branch\n");
    let reopened = store(&remote, "develop", &ws);
    reopened.ensure_repository().unwrap();
    branch_and_contents(&ws, "develop", "remote develop branch\n");
    std::fs::write(ws.join("branch.txt"), "local develop update\n").unwrap();
    commit(&reopened, "Update develop branch marker", &["branch.txt"]).unwrap();
    assert_eq!(head_branch(&ws), "develop");
    assert_eq!(
        remote_file(&remote, "develop", "branch.txt").unwrap(),
        "local develop update\n"
    );
    assert_eq!(
        remote_file(&remote, "master", "branch.txt").unwrap(),
        "remote master branch\n"
    );
}

#[test]
fn switches_to_a_configured_branch_created_after_clone() {
    let root = scratch("switch-new");
    let remote = setup_remote(&root, "master", &[("master", "remote master branch\n")]);
    let ws = root.join("workspace");
    store(&remote, "", &ws).ensure_repository().unwrap();
    branch_and_contents(&ws, "master", "remote master branch\n");
    advance(&root, "release/2026", "release branch\n", true);
    store(&remote, "release/2026", &ws).ensure_repository().unwrap();
    branch_and_contents(&ws, "release/2026", "release branch\n");
}

#[test]
fn resets_to_the_remote_default_when_branch_unset() {
    let root = scratch("reset-default");
    let remote = setup_remote(
        &root,
        "master",
        &[
            ("master", "remote master branch\n"),
            ("develop", "remote develop branch\n"),
        ],
    );
    let ws = root.join("workspace");
    store(&remote, "develop", &ws).ensure_repository().unwrap();
    branch_and_contents(&ws, "develop", "remote develop branch\n");
    let default = store(&remote, "", &ws);
    default.ensure_repository().unwrap();
    assert_eq!(head_branch(&ws), "master");
    std::fs::write(ws.join("branch.txt"), "local master update\n").unwrap();
    commit(&default, "Update master marker", &["branch.txt"]).unwrap();
    assert_eq!(
        remote_file(&remote, "master", "branch.txt").unwrap(),
        "local master update\n"
    );
}

#[test]
fn a_repeated_delete_does_not_overwrite_remote_only_changes() {
    let root = scratch("repeat-delete");
    let remote = setup_remote(&root, "master", &[("master", "remote master branch\n")]);
    let a = store(&remote, "", &root.join("workspace-a"));
    a.ensure_repository().unwrap();
    let path = save(&a, "a.json", r#"{"access_token":"a"}"#);
    a.delete(&path).unwrap();
    let b = store(&remote, "", &root.join("workspace-b"));
    b.ensure_repository().unwrap();
    save(&b, "b.json", r#"{"access_token":"b"}"#);
    assert!(remote_file(&remote, "master", "auths/b.json").is_some());
    a.delete(&path).unwrap();
    assert!(remote_file(&remote, "master", "auths/b.json").is_some());
}

#[test]
fn rejects_paths_outside_the_repository_before_mutation() {
    let root = scratch("outside");
    let remote = setup_remote(&root, "master", &[("master", "remote master branch\n")]);
    let s = store(&remote, "", &root.join("workspace"));
    s.ensure_repository().unwrap();
    let outside = root.join("outside.json");
    std::fs::write(&outside, "outside\n").unwrap();
    assert!(s.delete(&outside).is_err());
    assert_eq!(read(&outside), "outside\n");
    assert!(
        s.persist_auth_files("Sync auth outside.json", std::slice::from_ref(&outside))
            .is_err()
    );
    assert!(remote_file(&remote, "master", "outside.json").is_none());
}

#[test]
fn persist_config_drops_unrelated_staged_deletions() {
    let root = scratch("staged-delete");
    let remote = setup_remote(&root, "master", &[("master", "remote master branch\n")]);
    let ws = root.join("workspace");
    let s = store(&remote, "", &ws);
    s.ensure_repository().unwrap();
    let auth = save(&s, "protected.json", r#"{"access_token":"token"}"#);
    std::fs::write(s.config_path(), "version: one\n").unwrap();
    s.persist_config().unwrap();
    git(&ws, &["rm", "-q", "auths/protected.json"]);
    assert!(!auth.exists());
    std::fs::write(s.config_path(), "version: two\n").unwrap();
    s.persist_config().unwrap();
    assert!(remote_file(&remote, "master", "auths/protected.json").is_some());
    assert_eq!(
        remote_file(&remote, "master", "config/config.yaml").unwrap(),
        "version: two\n"
    );
}

#[test]
fn persist_config_repairs_the_index_after_an_unstaged_pull() {
    let root = scratch("unstaged-pull");
    let remote = setup_remote(&root, "master", &[("master", "remote master branch\n")]);
    let s = store(&remote, "", &root.join("workspace"));
    s.ensure_repository().unwrap();
    std::fs::write(s.config_path(), "source: local-config\n").unwrap();
    advance(&root, "master", "remote branch advanced\n", false);
    s.persist_config().unwrap();
    assert_eq!(
        remote_file(&remote, "master", "branch.txt").unwrap(),
        "remote branch advanced\n"
    );
    assert_eq!(
        remote_file(&remote, "master", "config/config.yaml").unwrap(),
        "source: local-config\n"
    );
}

#[test]
fn persist_config_preserves_remote_only_auth_after_divergence() {
    let root = scratch("divergence");
    let remote = setup_remote(&root, "master", &[("master", "remote master branch\n")]);
    let a = store(&remote, "", &root.join("workspace-a"));
    a.ensure_repository().unwrap();
    let b = store(&remote, "", &root.join("workspace-b"));
    b.ensure_repository().unwrap();
    save(&b, "remote-only.json", r#"{"access_token":"remote"}"#);
    std::fs::write(a.config_path(), "source: store-a\n").unwrap();
    a.persist_config().unwrap();
    assert!(remote_file(&remote, "master", "auths/remote-only.json").is_some());
    assert_eq!(
        remote_file(&remote, "master", "config/config.yaml").unwrap(),
        "source: store-a\n"
    );
}

/// Go `TestGitTokenStoreRejectsStaleForcePush` and
/// `TestGitTokenStoreSaveRetryAfterLeaseConflictCommitsMatchingContent`: a push whose
/// lease is stale is rejected without touching the remote; the retry, which pulls
/// first, lands.
#[test]
fn a_stale_lease_is_rejected_and_the_retry_lands() {
    let root = scratch("stale-lease");
    let remote = setup_remote(&root, "master", &[("master", "remote master branch\n")]);
    let a = store(&remote, "", &root.join("workspace-a"));
    a.ensure_repository().unwrap();
    let b = store(&remote, "", &root.join("workspace-b"));
    b.ensure_repository().unwrap();
    save(&b, "concurrent.json", r#"{"access_token":"remote"}"#);
    std::fs::write(a.config_path(), "source: stale-a\n").unwrap();
    assert!(commit(&a, "Update stale config", &["config/config.yaml"]).is_err());
    assert!(remote_file(&remote, "master", "auths/concurrent.json").is_some());
    assert!(remote_file(&remote, "master", "config/config.yaml").is_none());
    a.persist_config().unwrap();
    assert!(remote_file(&remote, "master", "auths/concurrent.json").is_some());
    assert_eq!(
        remote_file(&remote, "master", "config/config.yaml").unwrap(),
        "source: stale-a\n"
    );
}

/// Go `TestEnsureRepositoryRetryRestoresTrackedAuthOnUpToDatePull`: an unreachable
/// remote fails the sync without restoring anything; the retry restores the tracked
/// auth, which an explicit delete then removes.
#[test]
fn a_retry_restores_a_tracked_auth_on_an_up_to_date_pull() {
    let root = scratch("retry-restore");
    let remote = setup_remote(&root, "master", &[("master", "remote master branch\n")]);
    let ws = root.join("workspace");
    let s = store(&remote, "", &ws);
    s.ensure_repository().unwrap();
    let auth = save(&s, "retry.json", r#"{"access_token":"remote"}"#);
    git(&ws, &["rm", "-q", "auths/retry.json"]);
    let missing = root.join("missing.git").to_string_lossy().into_owned();
    git(&ws, &["remote", "set-url", "origin", &missing]);
    assert!(s.ensure_repository().is_err());
    assert!(!auth.exists());
    git(&ws, &["remote", "set-url", "origin", &remote]);
    s.ensure_repository().unwrap();
    assert_eq!(read(&auth), r#"{"access_token":"remote"}"#);
    s.delete(&auth).unwrap();
    assert!(remote_file(&remote, "master", "auths/retry.json").is_none());
    assert!(!auth.exists());
}

#[test]
fn reconciles_remote_auth_changes_around_local_config() {
    let root = scratch("reconcile-auth");
    let remote = setup_remote(&root, "master", &[("master", "remote master branch\n")]);
    let owner = store(&remote, "", &root.join("owner"));
    owner.ensure_repository().unwrap();
    for name in ["modified.json", "deleted.json"] {
        save(&owner, name, r#"{"access_token":"old"}"#);
    }
    std::fs::write(owner.config_path(), "source: original\n").unwrap();
    owner.persist_config().unwrap();
    let a = store(&remote, "", &root.join("workspace-a"));
    a.ensure_repository().unwrap();
    let b = store(&remote, "", &root.join("workspace-b"));
    b.ensure_repository().unwrap();
    std::fs::write(a.config_path(), "source: local-a\n").unwrap();
    save(&b, "modified.json", r#"{"access_token":"new"}"#);
    b.delete(&b.auth_dir().join("deleted.json")).unwrap();
    a.ensure_repository().unwrap();
    assert_eq!(read(&a.config_path()), "source: local-a\n");
    assert_eq!(read(&a.auth_dir().join("modified.json")), r#"{"access_token":"new"}"#);
    assert!(!a.auth_dir().join("deleted.json").exists());
}

#[test]
fn reconciles_remote_config_changes_around_local_auth() {
    let root = scratch("reconcile-config");
    let remote = setup_remote(&root, "master", &[("master", "remote master branch\n")]);
    let owner = store(&remote, "", &root.join("owner"));
    owner.ensure_repository().unwrap();
    save(&owner, "local.json", r#"{"access_token":"old"}"#);
    std::fs::write(owner.config_path(), "source: original\n").unwrap();
    owner.persist_config().unwrap();
    let a = store(&remote, "", &root.join("workspace-a"));
    a.ensure_repository().unwrap();
    let b = store(&remote, "", &root.join("workspace-b"));
    b.ensure_repository().unwrap();
    let local = a.auth_dir().join("local.json");
    let dirty = r#"{"type":"codex","access_token":"local-dirty"}"#;
    std::fs::write(&local, dirty).unwrap();
    std::fs::write(b.config_path(), "source: remote-modified\n").unwrap();
    b.persist_config().unwrap();
    a.ensure_repository().unwrap();
    assert_eq!(read(&a.config_path()), "source: remote-modified\n");
    assert_eq!(read(&local), dirty);

    std::fs::remove_file(b.config_path()).unwrap();
    commit(&b, "Delete config", &["config/config.yaml"]).unwrap();
    a.ensure_repository().unwrap();
    assert!(!a.config_path().exists());
    assert_eq!(read(&local), dirty);
}

#[test]
fn fails_closed_on_a_same_path_conflict() {
    let root = scratch("same-path");
    let remote = setup_remote(&root, "master", &[("master", "remote master branch\n")]);
    let owner = store(&remote, "", &root.join("owner"));
    owner.ensure_repository().unwrap();
    std::fs::write(owner.config_path(), "source: original\n").unwrap();
    owner.persist_config().unwrap();
    let a = store(&remote, "", &root.join("workspace-a"));
    a.ensure_repository().unwrap();
    let b = store(&remote, "", &root.join("workspace-b"));
    b.ensure_repository().unwrap();
    std::fs::write(a.config_path(), "source: local\n").unwrap();
    std::fs::write(b.config_path(), "source: remote\n").unwrap();
    b.persist_config().unwrap();
    let error = format!("{:#}", a.ensure_repository().unwrap_err());
    assert!(error.contains("conflicts with local change"), "{error}");
    assert_eq!(read(&a.config_path()), "source: local\n");
    assert_eq!(
        remote_file(&remote, "master", "config/config.yaml").unwrap(),
        "source: remote\n"
    );
}

/// Go `GitTokenStore.Save` order: sync, then write, then commit. A credential another
/// node changed is pulled before the new contents land, so re-saving it succeeds
/// where writing first would conflict with the remote change; a failed sync writes
/// nothing.
#[test]
fn save_syncs_before_writing_a_remotely_changed_credential() {
    let root = scratch("save-order");
    let remote = setup_remote(&root, "master", &[("master", "remote master branch\n")]);
    let a = store(&remote, "", &root.join("workspace-a"));
    a.ensure_repository().unwrap();
    save(&a, "vertex-p.json", r#"{"v":1}"#);
    let b = store(&remote, "", &root.join("workspace-b"));
    b.ensure_repository().unwrap();
    save(&a, "vertex-p.json", r#"{"v":2}"#);
    let path = b.auth_dir().join("vertex-p.json");
    let write = |contents: &'static str| -> cpa_server::persist::SaveFn<'static> {
        let path = path.clone();
        Box::new(move || {
            std::fs::write(&path, contents)?;
            Ok(Some(path))
        })
    };
    assert_eq!(b.save(write(r#"{"v":3}"#)).unwrap(), Some(path.clone()));
    assert_eq!(
        remote_file(&remote, "master", "auths/vertex-p.json").unwrap(),
        r#"{"v":3}"#
    );
    let log = git(Path::new(&remote), &["log", "-1", "--format=%s"]);
    assert_eq!(log, "Update auth vertex-p.json");

    // The same re-save written before the sync conflicts with the remote change.
    let a_path = a.auth_dir().join("vertex-p.json");
    a.save(Box::new(|| {
        std::fs::write(&a_path, r#"{"v":4}"#)?;
        Ok(Some(a_path.clone()))
    }))
    .unwrap();
    std::fs::write(&path, r#"{"v":5}"#).unwrap();
    let error = format!(
        "{:#}",
        b.persist_auth_files("Sync auth vertex-p.json", std::slice::from_ref(&path))
            .unwrap_err()
    );
    assert!(error.contains("conflicts with local change"), "{error}");
    git(&root.join("workspace-b"), &["checkout", "--", "auths/vertex-p.json"]);

    std::fs::rename(&remote, root.join("gone.git")).unwrap();
    let fresh = b.auth_dir().join("vertex-q.json");
    let called = std::cell::Cell::new(false);
    let error = b
        .save(Box::new(|| {
            called.set(true);
            Ok(Some(fresh.clone()))
        }))
        .unwrap_err();
    assert!(format!("{error:#}").contains("pull"), "{error:#}");
    assert!(!called.get() && !fresh.exists(), "a failed sync writes nothing");
}

/// Go `TestCommitAndPushLockedPushesBeforeRunningGC`: a due GC never delays the push.
#[test]
fn pushes_before_running_gc() {
    let root = scratch("gc");
    let remote = setup_remote(&root, "master", &[("master", "remote master branch\n")]);
    let ws = root.join("workspace");
    let s = store(&remote, "", &ws);
    s.ensure_repository().unwrap();
    for contents in ["local master update one\n", "local master update two\n"] {
        std::fs::write(ws.join("branch.txt"), contents).unwrap();
        *s.lock.lock().unwrap() = None;
        commit(&s, "Update master marker", &["branch.txt"]).unwrap();
        assert_eq!(remote_file(&remote, "master", "branch.txt").unwrap(), contents);
    }
}

#[test]
fn follows_a_renamed_remote_default_branch() {
    let root = scratch("renamed");
    let remote = setup_remote(
        &root,
        "master",
        &[("master", "remote master branch\n"), ("main", "remote main branch\n")],
    );
    let ws = root.join("workspace");
    store(&remote, "", &ws).ensure_repository().unwrap();
    branch_and_contents(&ws, "master", "remote master branch\n");
    git(Path::new(&remote), &["symbolic-ref", "HEAD", "refs/heads/main"]);
    advance(&root, "main", "remote main branch updated\n", false);
    store(&remote, "", &ws).ensure_repository().unwrap();
    branch_and_contents(&ws, "main", "remote main branch updated\n");
    assert_eq!(remote_head(&remote), "main");
}

/// Go `TestEnsureRepositoryKeepsCurrentBranchWhenRemoteDefaultCannotBeResolved`: a
/// remote that demands authentication leaves the pinned branch checked out.
#[test]
fn keeps_the_current_branch_when_the_remote_default_cannot_be_resolved() {
    use std::io::{Read, Write};
    let root = scratch("unresolved");
    let remote = setup_remote(
        &root,
        "master",
        &[
            ("master", "remote master branch\n"),
            ("develop", "remote develop branch\n"),
        ],
    );
    let ws = root.join("workspace");
    store(&remote, "develop", &ws).ensure_repository().unwrap();
    branch_and_contents(&ws, "develop", "remote develop branch\n");
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let url = format!("http://{}/tokens.git", listener.local_addr().unwrap());
    std::thread::spawn(move || {
        for stream in listener.incoming() {
            let Ok(mut stream) = stream else { return };
            let mut buf = [0u8; 4096];
            let _ = stream.read(&mut buf);
            let _ = stream.write_all(
                b"HTTP/1.1 401 Unauthorized\r\nWWW-Authenticate: Basic realm=\"git\"\r\nContent-Length: 13\r\nConnection: close\r\n\r\nauth required",
            );
        }
    });
    git(&ws, &["remote", "set-url", "origin", &url]);
    store(&remote, "", &ws).ensure_repository().unwrap();
    assert_eq!(head_branch(&ws), "develop");
}

/// `git mktree` of `entries` (`<mode> <type> <hash>\t<name>` lines) in `dir`: git's
/// plumbing builds trees that its checkout would refuse, as a hostile remote can.
fn mktree(dir: &Path, entries: &str) -> String {
    use std::io::Write;
    let mut child = Command::new("git")
        .current_dir(dir)
        .arg("mktree")
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .spawn()
        .unwrap();
    child.stdin.take().unwrap().write_all(entries.as_bytes()).unwrap();
    let output = child.wait_with_output().unwrap();
    assert!(output.status.success());
    String::from_utf8_lossy(&output.stdout).trim().to_owned()
}

/// A commit in `ws` whose tree holds `safe.txt` plus `.git/evil` and `.GIT/evil`.
fn hostile_commit(ws: &Path) -> String {
    std::fs::write(ws.join("payload"), "[core]\n\tfsmonitor = touch pwned\n").unwrap();
    let blob = git(ws, &["hash-object", "-w", "payload"]);
    std::fs::remove_file(ws.join("payload")).unwrap();
    let sub = mktree(ws, &format!("100644 blob {blob}\tevil\n"));
    let tree = mktree(
        ws,
        &format!("040000 tree {sub}\t.git\n040000 tree {sub}\t.GIT\n100644 blob {blob}\tsafe.txt\n"),
    );
    git(ws, &["commit-tree", &tree, "-m", "hostile tree"])
}

/// cliproxy-rs-7ea.3: a tracked path that git's checkout refused is never restored by
/// hand into `.git` (in any case).
#[test]
fn restore_missing_skips_paths_inside_the_git_directory() {
    let root = scratch("hostile-restore");
    let remote = setup_remote(&root, "master", &[("master", "remote master branch\n")]);
    let ws = root.join("workspace");
    let s = store(&remote, "", &ws);
    s.ensure_repository().unwrap();
    let commit = hostile_commit(&ws);
    git(&ws, &["update-ref", "refs/heads/master", &commit]);
    s.restore_missing().unwrap();
    assert!(ws.join("safe.txt").exists(), "a plain tracked file is restored");
    assert!(!ws.join(".git/evil").exists());
    assert!(!ws.join(".GIT").exists());
}

/// cliproxy-rs-7ea.3: the pull's reconcile writes the remote's plain files but no path
/// inside `.git`.
#[test]
fn reconcile_skips_paths_inside_the_git_directory() {
    let root = scratch("hostile-reconcile");
    let remote = setup_remote(&root, "master", &[("master", "remote master branch\n")]);
    let ws = root.join("workspace");
    let s = store(&remote, "", &ws);
    s.ensure_repository().unwrap();
    let base = git(&ws, &["rev-parse", "HEAD"]);
    let commit = hostile_commit(&ws);
    // The files are written before the index reset, which git may refuse for this tree
    // (`invalid path '.git/evil'`); only what reached the disk matters here.
    let _ = s.reconcile(Some(&base), &commit, "refs/heads/master");
    assert!(ws.join("safe.txt").exists());
    assert!(!ws.join(".git/evil").exists());
    assert!(!ws.join(".GIT").exists());
}

/// cliproxy-rs-7ea.23: a remote default branch that git would parse as an option is
/// refused, and the store keeps its current branch as it does for an unresolvable one.
#[test]
fn an_option_like_remote_default_branch_is_refused() {
    let root = scratch("dash-branch");
    let remote = setup_remote(&root, "master", &[("master", "remote master branch\n")]);
    let ws = root.join("workspace");
    store(&remote, "", &ws).ensure_repository().unwrap();
    let tip = git(Path::new(&remote), &["rev-parse", "refs/heads/master"]);
    git(Path::new(&remote), &["update-ref", "refs/heads/--orphan", &tip]);
    git(Path::new(&remote), &["symbolic-ref", "HEAD", "refs/heads/--orphan"]);
    let s = store(&remote, "", &ws);
    assert!(s.remote_default_branch().is_err());
    s.ensure_repository().unwrap();
    assert_eq!(head_branch(&ws), "master");
}

/// cliproxy-rs-zlj.8: a remote whose history gained a tree entry named `.git` is refused
/// by the fetch's fsck (`hasDotgit` raised to an error) instead of warning and relying on
/// `safe_tree_path` alone.
#[test]
fn a_fetch_bringing_a_dotgit_tree_entry_is_refused() {
    let root = scratch("fsck-dotgit");
    let remote = setup_remote(&root, "master", &[("master", "remote master branch\n")]);
    let a = store(&remote, "", &root.join("workspace-a"));
    a.ensure_repository().unwrap();
    // Built in another workspace, so the fetch has to transfer the objects.
    let owner_ws = root.join("owner");
    let owner = store(&remote, "", &owner_ws);
    owner.ensure_repository().unwrap();
    let commit = hostile_commit(&owner_ws);
    git(
        &owner_ws,
        &["push", "-q", "-f", "origin", &format!("{commit}:refs/heads/master")],
    );
    let error = format!("{:#}", a.ensure_repository().unwrap_err());
    assert!(error.contains("pull"), "{error}");
    assert!(error.contains("hasDotgit"), "{error}");
    assert!(!root.join("workspace-a/.git/evil").exists());
    assert!(
        !root.join("workspace-a/safe.txt").exists(),
        "nothing of the refused tip lands"
    );
}

/// The requests a mock HTTP server saw: `(request target, Authorization header value)`.
type Seen = std::sync::Arc<std::sync::Mutex<Vec<(String, Option<String>)>>>;

/// A loopback HTTP server standing in for a git remote: `(base URL, requests seen)`.
/// `respond(own base URL, target, Authorization)` gives `(status line, extra header
/// lines, body)`; every response closes the connection.
fn mock_http(
    respond: impl Fn(&str, &str, Option<&str>) -> (&'static str, String, String) + Send + 'static,
) -> (String, Seen) {
    use std::io::{Read, Write};
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    let seen: Seen = Default::default();
    let served = seen.clone();
    let own = base.clone();
    std::thread::spawn(move || {
        for stream in listener.incoming() {
            let Ok(mut stream) = stream else { return };
            let mut head = Vec::new();
            let mut buf = [0u8; 1024];
            while !head.windows(4).any(|w| w == b"\r\n\r\n") {
                let Ok(n) = stream.read(&mut buf) else { break };
                if n == 0 {
                    break;
                }
                head.extend_from_slice(&buf[..n]);
            }
            let text = String::from_utf8_lossy(&head).into_owned();
            let target = text.split_whitespace().nth(1).unwrap_or_default().to_owned();
            let authorization = text
                .lines()
                .find_map(|line| line.strip_prefix("Authorization: "))
                .map(|value| value.trim_end().to_owned());
            served.lock().unwrap().push((target.clone(), authorization.clone()));
            let (status, headers, body) = respond(&own, &target, authorization.as_deref());
            let _ = stream.write_all(
                format!(
                    "HTTP/1.1 {status}\r\n{headers}Content-Length: {}\r\nConnection: close\r\n\r\n{body}",
                    body.len()
                )
                .as_bytes(),
            );
        }
    });
    (base, seen)
}

const MOCK_HASH: &str = "0123456789abcdef0123456789abcdef01234567";

/// A dumb-HTTP remote under `/tokens.git/` (`info/refs` is a plain list, so no pack is
/// needed) that challenges every request without the `expected` Basic credential; a
/// `/tokens/...` target (the remote written without its `.git` suffix) redirects to
/// `redirect_base/tokens.git/...`.
fn dumb_remote(
    redirect_base: &str,
    target: &str,
    authorization: Option<&str>,
    expected: &str,
) -> (&'static str, String, String) {
    if let Some(rest) = target.strip_prefix("/tokens/") {
        return (
            "301 Moved Permanently",
            format!("Location: {redirect_base}/tokens.git/{rest}\r\n"),
            String::new(),
        );
    }
    if authorization != Some(expected) {
        return (
            "401 Unauthorized",
            "WWW-Authenticate: Basic realm=\"tokens\"\r\n".to_owned(),
            String::new(),
        );
    }
    if target.starts_with("/tokens.git/info/refs") {
        (
            "200 OK",
            "Content-Type: text/plain\r\n".to_owned(),
            format!("{MOCK_HASH}\trefs/heads/main\n"),
        )
    } else if target == "/tokens.git/HEAD" {
        (
            "200 OK",
            "Content-Type: text/plain\r\n".to_owned(),
            "ref: refs/heads/main\n".to_owned(),
        )
    } else {
        ("404 Not Found", String::new(), String::new())
    }
}

/// The Basic credential git derives from user `git` and `secret-token`.
fn expected_basic() -> String {
    use base64::Engine;
    format!(
        "Basic {}",
        base64::engine::general_purpose::STANDARD.encode("git:secret-token")
    )
}

/// cliproxy-rs-zlj.2: with a token configured, a same-host redirect (a GitLab remote
/// written without its `.git` suffix) is followed on the first request, the later
/// requests go to the URL it landed on, and the credential arrives there as Basic auth
/// once that host has challenged. `http.followRedirects=false` refused the redirect.
#[test]
fn a_same_host_redirect_is_followed_with_the_token() {
    let root = scratch("redirect");
    let ws = root.join("workspace");
    std::fs::create_dir_all(&ws).unwrap();
    let expected = expected_basic();
    let (base, seen) = {
        let expected = expected.clone();
        mock_http(move |own, target, authorization| dumb_remote(own, target, authorization, &expected))
    };
    let url = format!("{base}/tokens");
    let s = GitStore::new(&url, "", "secret-token", "", &ws);
    let listed = s.run_in(&ws, &["ls-remote", "--", &url]).unwrap();
    assert!(listed.status.success(), "{}", String::from_utf8_lossy(&listed.stderr));
    let out = String::from_utf8_lossy(&listed.stdout);
    assert!(out.contains(&format!("{MOCK_HASH}\trefs/heads/main")), "{out}");
    let seen = seen.lock().unwrap();
    assert!(seen.len() >= 2, "{seen:?}");
    assert!(
        seen[0].0.starts_with("/tokens/info/refs") && seen[0].1.is_none(),
        "the first request carries no credential before a challenge: {seen:?}"
    );
    assert!(
        seen[1..].iter().all(|(target, _)| target.starts_with("/tokens.git/")),
        "later requests go to the URL the redirect landed on: {seen:?}"
    );
    assert!(
        seen.iter()
            .any(|(target, auth)| target.starts_with("/tokens.git/info/refs") && auth.as_deref() == Some(&*expected)),
        "the credential arrives at the rebased same-host URL: {seen:?}"
    );
    assert!(
        seen.iter()
            .all(|(_, auth)| auth.is_none() || auth.as_deref() == Some(&*expected)),
        "{seen:?}"
    );
}

/// cliproxy-rs-zlj.2: the credential is scoped to the configured remote's host and port,
/// so a first request redirected to another host leaves that host without any
/// Authorization header; git has nothing to offer it and the command fails there.
#[test]
fn a_cross_host_redirect_gets_no_credential() {
    let root = scratch("cross-redirect");
    let ws = root.join("workspace");
    std::fs::create_dir_all(&ws).unwrap();
    let expected = expected_basic();
    // The host the redirect lands on: another loopback port, which is another host to
    // git's credential matching.
    let (other, other_seen) = {
        let expected = expected.clone();
        mock_http(move |own, target, authorization| dumb_remote(own, target, authorization, &expected))
    };
    // The configured remote: every request is sent on to the other host.
    let (base, seen) = {
        let other = other.clone();
        mock_http(move |_, target, _| {
            let rest = target.strip_prefix("/tokens/").unwrap_or(target);
            (
                "301 Moved Permanently",
                format!("Location: {other}/tokens.git/{rest}\r\n"),
                String::new(),
            )
        })
    };
    let url = format!("{base}/tokens");
    let s = GitStore::new(&url, "", "secret-token", "", &ws);
    let listed = s.run_in(&ws, &["ls-remote", "--", &url]).unwrap();
    assert!(!listed.status.success(), "{}", String::from_utf8_lossy(&listed.stdout));
    let other_seen = other_seen.lock().unwrap();
    assert!(!other_seen.is_empty(), "the redirect was followed");
    assert!(
        other_seen.iter().all(|(_, auth)| auth.is_none()),
        "no credential reaches the other host: {other_seen:?}"
    );
    let seen = seen.lock().unwrap();
    assert!(seen.iter().all(|(_, auth)| auth.is_none()), "{seen:?}");
    assert!(
        !String::from_utf8_lossy(&listed.stderr).contains("secret-token"),
        "the token is in no error text"
    );
}

/// cliproxy-rs-zlj.7: a git killed at the deadline leaves its lock files behind; the
/// ones it created are removed, so the next save proceeds, while a lock older than the
/// killed command is left alone.
#[test]
fn locks_left_by_a_killed_git_are_removed() {
    use std::process::Stdio;
    use std::time::{Duration, SystemTime};
    let root = scratch("killed-locks");
    let remote = setup_remote(&root, "master", &[("master", "remote master branch\n")]);
    let ws = root.join("workspace");
    let s = store(&remote, "", &ws);
    s.ensure_repository().unwrap();
    let elder = ws.join(".git/refs/heads/elder.lock");
    std::fs::write(&elder, "").unwrap();
    std::fs::File::options()
        .write(true)
        .open(&elder)
        .unwrap()
        .set_modified(SystemTime::now() - Duration::from_secs(600))
        .unwrap();
    // Stands in for a git that took its locks and then stalled.
    let mut stalled = Command::new("sh");
    stalled
        .current_dir(&ws)
        .args([
            "-c",
            "touch .git/index.lock .git/packed-refs.lock .git/refs/heads/master.lock; sleep 30",
        ])
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let error = super::output_with_timeout_in(&ws, &mut stalled, Duration::from_millis(500)).unwrap_err();
    assert_eq!(error.kind(), std::io::ErrorKind::TimedOut);
    for lock in ["index.lock", "packed-refs.lock", "refs/heads/master.lock"] {
        assert!(!ws.join(".git").join(lock).exists(), "{lock} is removed");
    }
    assert!(elder.exists(), "a lock older than the killed command is kept");
    std::fs::remove_file(&elder).unwrap();
    save(&s, "after.json", r#"{"access_token":"after"}"#);
    assert!(remote_file(&remote, "master", "auths/after.json").is_some());
}

/// cliproxy-rs-7ea.23: a stalled git is killed at the deadline; a finished one returns
/// both streams.
#[test]
fn git_subprocesses_are_killed_at_the_deadline() {
    use std::process::Stdio;
    use std::time::{Duration, Instant};
    let started = Instant::now();
    let mut slow = Command::new("sh");
    slow.args(["-c", "sleep 30"])
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let error = super::output_with_timeout(&mut slow, Duration::from_millis(200)).unwrap_err();
    assert_eq!(error.kind(), std::io::ErrorKind::TimedOut);
    assert!(started.elapsed() < Duration::from_secs(10), "{:?}", started.elapsed());
    let mut quick = Command::new("sh");
    quick
        .args(["-c", "printf out; printf err >&2; exit 3"])
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let output = super::output_with_timeout(&mut quick, Duration::from_secs(30)).unwrap();
    assert_eq!(output.status.code(), Some(3));
    assert_eq!(output.stdout, b"out");
    assert_eq!(output.stderr, b"err");
}
