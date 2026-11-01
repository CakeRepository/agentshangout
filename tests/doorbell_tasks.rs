//! Tests for how the doorbell runs an owner's task (OT6, OT7), against a tiny fake worker: a script
//! that reports what it was given. The rules are in docs/routines.md. A comment such as `// OT7`
//! above a test names the rule it covers.

use std::{
    fs,
    path::{Path, PathBuf},
    time::{Duration, Instant},
};

use agenthangout::doorbell::{pr_url, run_task, TaskOutcome};

/// A worker is one program path, so on Windows it is a `.cmd` and elsewhere a shell script.
fn worker(name: &str, body: &[&str]) -> PathBuf {
    let dir = Path::new(env!("CARGO_TARGET_TMPDIR")).join("doorbell-tasks");
    fs::create_dir_all(&dir).unwrap();
    let path = dir.join(if cfg!(windows) { format!("{name}.cmd") } else { format!("{name}.sh") });
    let mut script = if cfg!(windows) { "@echo off\r\n" } else { "#!/bin/sh\n" }.to_string();
    for line in body {
        script.push_str(line);
        script.push_str(if cfg!(windows) { "\r\n" } else { "\n" });
    }
    fs::write(&path, script).unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&path, fs::Permissions::from_mode(0o755)).unwrap();
    }
    path
}

/// The doorbell has its own token and address in its environment, as it does when it is run.
fn doorbell_environment() {
    std::env::set_var("AGENTHANGOUT_TOKEN", "ah_bot_the-doorbells-own-token");
    std::env::set_var("AGENTHANGOUT_URL", "https://example.test");
    std::env::set_var("PLANTED_SECRET", "hunter2-planted");
}

// OT7
#[tokio::test]
async fn the_worker_gets_the_task_on_stdin_and_never_the_token() {
    doorbell_environment();
    let report = if cfg!(windows) {
        worker(
            "report",
            &[
                "echo token=[%AGENTHANGOUT_TOKEN%]",
                "echo url=[%AGENTHANGOUT_URL%]",
                "echo id=[%TASK_ID%]",
                "echo secret=[%PLANTED_SECRET%]",
                "echo repo=[%AGENTHANGOUT_TASK_REPO%]",
                "findstr \"^\"",
                "echo https://github.com/a/b/pull/5",
            ],
        )
    } else {
        worker(
            "report",
            &[
                "echo \"token=[$AGENTHANGOUT_TOKEN]\"",
                "echo \"url=[$AGENTHANGOUT_URL]\"",
                "echo \"id=[$TASK_ID]\"",
                "echo \"secret=[$PLANTED_SECRET]\"",
                "echo \"repo=[$AGENTHANGOUT_TASK_REPO]\"",
                "cat",
                "echo",
                "echo https://github.com/a/b/pull/5",
            ],
        )
    };
    let outcome =
        run_task(&report, "room-12", "fix the typo\nthen stop", Duration::from_secs(60), "a/b")
            .await;
    let TaskOutcome::Done(output) = outcome else { panic!("{outcome:?}") };
    let lines: Vec<_> = output.lines().map(str::trim).collect();
    let expected = ["token=[]", "url=[]", "id=[room-12]", "secret=[]", "repo=[a/b]"];
    assert_eq!(lines[..5], expected, "{output}");
    assert_eq!(lines[5], "fix the typo", "{output}");
    assert_eq!(lines[6], "then stop", "{output}");
    assert!(
        !output.contains("hunter2-planted")
            && !output.contains("the-doorbells-own-token")
            && !output.contains("example.test")
    );
    assert_eq!(pr_url(&output, "a/b").as_deref(), Some("https://github.com/a/b/pull/5"));
}

// OT6
#[tokio::test]
async fn a_worker_that_fails_or_cant_start_is_a_failure() {
    let fails = worker("fails", &["exit 3"]);
    let outcome = run_task(&fails, "x", "t", Duration::from_secs(60), "a/b").await;
    assert!(matches!(outcome, TaskOutcome::Failed(_)), "{outcome:?}");

    let missing = Path::new(env!("CARGO_TARGET_TMPDIR")).join("no-such-worker");
    let outcome = run_task(&missing, "x", "t", Duration::from_secs(60), "a/b").await;
    assert!(matches!(outcome, TaskOutcome::Failed(_)), "{outcome:?}");
}

// OT6
#[tokio::test]
async fn a_worker_that_runs_too_long_is_killed() {
    let slow = if cfg!(windows) {
        worker("slow", &["ping -n 60 127.0.0.1 >nul"])
    } else {
        worker("slow", &["sleep 60"])
    };
    let started = Instant::now();
    let outcome = run_task(&slow, "x", "t", Duration::from_secs(1), "a/b").await;
    assert_eq!(outcome, TaskOutcome::TimedOut);
    assert!(started.elapsed() < Duration::from_secs(30), "{:?}", started.elapsed());
}

// A task worker must never automatically delete other, active, or unpublished worktrees.
#[test]
fn the_worker_preserves_worktrees_for_explicit_owner_cleanup() {
    let script = include_str!("../hooks/task-worker.ps1");
    assert!(!script.contains("Remove-Item"));
    assert!(!script.contains("worktree remove"));
    assert!(!script.contains("branch -D"));
}
