use parvi_mission_engine::Workspaces;
use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
    process::Command,
};

struct Scratch(PathBuf);
impl Scratch {
    fn new() -> Self {
        let mut bytes = [0; 8];
        getrandom::fill(&mut bytes).unwrap();
        let root = std::env::temp_dir().join(format!(
            "parvi-103-workspaces-{}",
            u64::from_ne_bytes(bytes)
        ));
        std::fs::create_dir_all(root.join("checkout")).unwrap();
        let repo = root.join("checkout");
        git(&repo, &["init", "-b", "main"]);
        git(&repo, &["config", "user.name", "Test"]);
        git(&repo, &["config", "user.email", "test@example.invalid"]);
        std::fs::write(repo.join(".gitignore"), ".env\n").unwrap();
        git(&repo, &["add", ".gitignore"]);
        git(&repo, &["commit", "-m", "base"]);
        Self(root)
    }
    fn repo(&self) -> PathBuf {
        self.0.join("checkout")
    }
    fn workspaces(&self) -> Workspaces {
        Workspaces::new(
            self.0.join("worktrees"),
            BTreeMap::from([("owner/repo".into(), self.repo())]),
        )
    }
    fn failing_hook(&self) {
        let hook = self.repo().join(".git/hooks/post-checkout");
        std::fs::write(&hook, "#!/bin/sh\nexit 1\n").unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&hook, std::fs::Permissions::from_mode(0o755)).unwrap();
        }
    }
}
impl Drop for Scratch {
    fn drop(&mut self) {
        std::fs::remove_dir_all(&self.0).unwrap();
    }
}
fn git(repo: &Path, args: &[&str]) -> String {
    let output = Command::new("git")
        .current_dir(repo)
        .args(args)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "git {args:?}: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout).unwrap().trim().to_owned()
}
#[test]
fn landing_preserves_ignored_files_until_force() {
    let scratch = Scratch::new();
    let workspaces = scratch.workspaces();
    let tree = workspaces.prepare("run_ignored", "owner/repo", "").unwrap();
    std::fs::write(tree.path.join(".env"), "valuable").unwrap();
    let error = workspaces
        .remove(&tree, false, &Default::default())
        .unwrap_err();
    assert!(error.to_string().contains("ignored"));
    assert_eq!(
        std::fs::read_to_string(tree.path.join(".env")).unwrap(),
        "valuable"
    );
    assert!(!git(&scratch.repo(), &["branch", "--list", &tree.branch]).is_empty());
    workspaces.remove(&tree, true, &Default::default()).unwrap();
    assert!(!tree.path.exists());
}
#[test]
fn failed_add_removes_its_worktree_and_branch_but_preserves_existing_resources() {
    let scratch = Scratch::new();
    let workspaces = scratch.workspaces();
    scratch.failing_hook();
    assert!(workspaces.prepare("run_hook", "owner/repo", "").is_err());
    assert!(!scratch.0.join("worktrees/run_hook").exists());
    assert!(git(&scratch.repo(), &["branch", "--list", "parvi/run-run_hook"]).is_empty());
    assert_eq!(
        git(&scratch.repo(), &["worktree", "list", "--porcelain"])
            .matches("worktree ")
            .count(),
        1
    );
    std::fs::create_dir(scratch.0.join("worktrees/run_existing")).unwrap();
    std::fs::write(scratch.0.join("worktrees/run_existing/keep"), "keep").unwrap();
    assert!(
        workspaces
            .prepare("run_existing", "owner/repo", "")
            .is_err()
    );
    assert!(scratch.0.join("worktrees/run_existing/keep").exists());
    git(&scratch.repo(), &["branch", "parvi/run-run_branch"]);
    assert!(workspaces.prepare("run_branch", "owner/repo", "").is_err());
    assert!(
        !git(
            &scratch.repo(),
            &["branch", "--list", "parvi/run-run_branch"]
        )
        .is_empty()
    );
    assert!(!scratch.0.join("worktrees/run_branch").exists());
}
#[test]
fn failed_landing_checkout_cleans_its_temporary_worktree() {
    let scratch = Scratch::new();
    let workspaces = scratch.workspaces();
    let tree = workspaces.prepare("run_land", "owner/repo", "").unwrap();
    let temporary = scratch.0.join("worktrees/.land-parvi-run-run_land");
    std::fs::create_dir(&temporary).unwrap();
    std::fs::write(temporary.join("keep"), "keep").unwrap();
    assert!(
        workspaces
            .remove(&tree, false, &Default::default())
            .is_err()
    );
    assert!(temporary.join("keep").exists());
    std::fs::remove_dir_all(&temporary).unwrap();
    scratch.failing_hook();
    assert!(
        workspaces
            .remove(&tree, false, &Default::default())
            .is_err()
    );
    assert!(tree.path.exists());
    assert!(
        !scratch
            .0
            .join("worktrees/.land-parvi-run-run_land")
            .exists()
    );
    assert_eq!(
        git(&scratch.repo(), &["worktree", "list", "--porcelain"])
            .matches("worktree ")
            .count(),
        2
    );
    assert!(!git(&scratch.repo(), &["branch", "--list", &tree.branch]).is_empty());
    std::fs::remove_file(scratch.repo().join(".git/hooks/post-checkout")).unwrap();
    workspaces
        .remove(&tree, false, &Default::default())
        .unwrap();
}
