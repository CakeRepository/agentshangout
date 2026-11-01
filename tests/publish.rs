//! Tests for the first step of the deploy job in .github/workflows/publish.yml, which stops an older
//! commit going out over a newer one. Publish only runs after a merge, so this takes the step's text
//! from the workflow and runs it the way Actions does, with a stand-in for gh. It also holds which
//! of the workflow's jobs a newer merge can cancel.

#![cfg(unix)]

use std::process::{Command, Output};

const WORKFLOW: &str = include_str!("../.github/workflows/publish.yml");
const STEP: &str = "- name: Refuse anything older than what's live";
const ONLY_IF_INSTALLING: &str = "if: steps.order.outputs.install == 'true'";

/// Answers only what the step should ask. `$LIVE` is empty when there's no full release, and a call
/// that starts with `$DOWN` fails the way gh does when GitHub can't answer. What the step writes to
/// `$GITHUB_OUTPUT` comes out on stdout when it ends.
const STAND_IN: &str = r#"
GITHUB_OUTPUT=$(mktemp)
trap 'cat "$GITHUB_OUTPUT"; rm "$GITHUB_OUTPUT"' EXIT
gh() {
    case "$*" in
        "$DOWN"*) echo "gh: Bad Gateway (HTTP 502)" >&2; return 1 ;;
        "release list --exclude-drafts --exclude-pre-releases "*) echo "$LIVE" ;;
        "release view "*) [ -n "$LIVE" ] || { echo "release not found" >&2; return 1; }; echo "$LIVE" ;;
        "api repos/$GH_REPO/compare/$LIVE...$GITHUB_SHA "*) echo "$ORDER" ;;
        *) echo "didn't expect gh $*" >&2; return 1 ;;
    esac
}
"#;

/// The step's script, which ends at the first line indented less than it is.
fn script() -> String {
    let (_, rest) = WORKFLOW.split_once(STEP).unwrap();
    let (_, body) = rest.split_once("run: |").unwrap();
    let lines: Vec<&str> = body
        .lines()
        .skip(1)
        .take_while(|line| line.is_empty() || line.starts_with("          "))
        .collect();
    assert!(!lines.is_empty());
    lines.join("\n")
}

/// Actions runs a step with `bash -e`.
fn run(live: &str, order: &str, down: &str) -> Output {
    Command::new("bash")
        .args(["-e", "-c", &format!("{STAND_IN}{}", script())])
        .env("TAG", "v0.1.67")
        .env("GH_REPO", "CakeRepository/agentshangout")
        .env("GITHUB_SHA", "eed0319")
        .env("LIVE", live)
        .env("ORDER", order)
        .env("DOWN", down)
        .output()
        .unwrap()
}

fn check(live: &str, order: &str) -> Output {
    run(live, order, "nothing is down")
}

fn stdout(output: &Output) -> String {
    String::from_utf8_lossy(&output.stdout).into_owned()
}

fn stderr(output: &Output) -> String {
    String::from_utf8_lossy(&output.stderr).into_owned()
}

fn installs(output: &Output) -> bool {
    stdout(output).lines().any(|line| line == "install=true")
}

#[track_caller]
fn goes_on(output: &Output) {
    assert!(output.status.success(), "{}{}", stdout(output), stderr(output));
    assert!(installs(output), "{}", stdout(output));
}

#[track_caller]
fn stops_with_nothing_to_chase(output: &Output) {
    assert!(output.status.success(), "{}{}", stdout(output), stderr(output));
    assert!(!installs(output), "{}", stdout(output));
    assert!(!stdout(output).contains("::error::"), "{}", stdout(output));
}

#[track_caller]
fn fails(output: &Output) {
    assert!(!output.status.success(), "{}", stdout(output));
    assert!(!installs(output), "{}", stdout(output));
}

// What happened on 2026-10-31, when v0.1.67 went out two minutes after v0.1.68.
#[test]
fn an_older_commit_stops_without_failing_the_run() {
    let stopped = check("v0.1.68", "behind");
    stops_with_nothing_to_chase(&stopped);
    assert!(stdout(&stopped).contains("::notice::"), "{}", stdout(&stopped));
    assert!(stdout(&stopped).contains("v0.1.67"), "{}", stdout(&stopped));
    assert!(stdout(&stopped).contains("v0.1.68"), "{}", stdout(&stopped));
}

// A revert is here too: it's a new commit, so it's ahead of the release it undoes.
#[test]
fn a_newer_commit_goes_on() {
    goes_on(&check("v0.1.68", "ahead"));
}

// A re-run of the run that's live, after its check of the site failed, say.
#[test]
fn the_commit_thats_live_can_go_out_again() {
    goes_on(&check("v0.1.68", "identical"));
}

// A prerelease left by a first deploy that failed isn't live either, so the next run ends up here.
#[test]
fn the_first_release_goes_on_with_nothing_to_compare_it_with() {
    goes_on(&check("", ""));
}

// main only moves forward, so this means something a person should look at.
#[test]
fn a_commit_that_isnt_older_or_newer_fails_the_run() {
    let failed = check("v0.1.68", "diverged");
    fails(&failed);
    assert!(stdout(&failed).contains("::error::"), "{}", stdout(&failed));
    assert!(stdout(&failed).contains("diverged"), "{}", stdout(&failed));
}

// Not hearing back isn't the same as there being nothing live.
#[test]
fn a_run_that_cant_tell_whats_live_fails() {
    for call in ["release list", "release view", "api"] {
        fails(&run("v0.1.68", "ahead", call));
    }
}

// A group on the deploy job let a run waiting for a person hold up the newer ones, which is how
// v0.1.67 went out over v0.1.68. A group on the whole workflow would cancel runs that are waiting.
#[test]
fn a_newer_merge_can_only_cancel_a_build() {
    let (before_deploy, deploy) = WORKFLOW.split_once("\n  deploy:\n").unwrap();
    let (top, build) = before_deploy.split_once("\n  build:\n").unwrap();
    assert!(build.contains("cancel-in-progress: true"));
    assert!(!top.contains("concurrency:"));
    assert!(!deploy.contains("concurrency:"));
}

// Skipping a step is what stops the deploy, so one without this would run for an older commit.
#[test]
fn every_step_after_the_check_depends_on_it() {
    let (_, rest) = WORKFLOW.split_once(STEP).unwrap();
    let steps: Vec<&str> = rest.split("\n      - ").skip(1).collect();
    assert!(!steps.is_empty());
    for step in steps {
        assert!(step.contains(ONLY_IF_INSTALLING), "this step runs anyway: {step}");
    }
}
