//! Tests for deploy/deploy.sh, run on a copy that points at a folder of its own, with stand-ins for
//! curl, systemctl and sleep. The Publish workflow checks the order of a release too, but only this
//! script sees what is really installed, so this is where the order is held.

#![cfg(unix)]

use agenthangout::auth;
use sha2::{Digest, Sha256};
use std::{
    fs::{self, File},
    path::PathBuf,
    process::{Command, Output},
};

/// The download writes the tag as the binary, and the health check fails while `$bin.down` exists.
const STAND_INS: &str = r#"
curl() {
    case "$*" in
        *releases/download/*) printf %s "$tag" > "$work/agenthangout" ;;
        *) [ ! -e "$bin.down" ] ;;
    esac
}
systemctl() { echo "$*" >> "$bin.systemctl"; }
sleep() { :; }
"#;

struct Server {
    dir: PathBuf,
}

impl Server {
    fn new() -> Self {
        let dir = std::env::temp_dir().join(format!("deploy-test-{}", auth::new_id()));
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join("agenthangout"), "the binary it started with").unwrap();
        let script = fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/deploy/deploy.sh"))
            .unwrap()
            .replacen("set -eu\n", &format!("set -eu\n{STAND_INS}"), 1)
            .replace("/usr/local/bin/agenthangout", dir.join("agenthangout").to_str().unwrap())
            .replace("/run/agenthangout-deploy.lock", dir.join("lock").to_str().unwrap());
        fs::write(dir.join("deploy.sh"), script).unwrap();
        Self { dir }
    }

    fn run(&self, args: &[&str]) -> Output {
        Command::new("sh").arg(self.dir.join("deploy.sh")).args(args).output().unwrap()
    }

    fn deploy(&self, tag: &str) -> Output {
        self.run(&[tag, &sha256(tag)])
    }

    fn roll_back(&self, tag: &str) -> Output {
        self.run(&[tag, &sha256(tag), "rollback"])
    }

    /// What's installed, which the stand-in download wrote as the tag.
    fn binary(&self) -> String {
        fs::read_to_string(self.dir.join("agenthangout")).unwrap()
    }

    fn restarts(&self) -> usize {
        let log = fs::read_to_string(self.dir.join("agenthangout.systemctl")).unwrap_or_default();
        log.lines().filter(|line| *line == "restart agenthangout").count()
    }

    fn set_down(&self, down: bool) {
        let file = self.dir.join("agenthangout.down");
        if down {
            fs::write(file, "").unwrap();
        } else {
            fs::remove_file(file).unwrap();
        }
    }
}

impl Drop for Server {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.dir);
    }
}

fn sha256(text: &str) -> String {
    Sha256::digest(text).iter().map(|byte| format!("{byte:02x}")).collect()
}

fn stdout(output: &Output) -> String {
    String::from_utf8_lossy(&output.stdout).into_owned()
}

fn stderr(output: &Output) -> String {
    String::from_utf8_lossy(&output.stderr).into_owned()
}

#[track_caller]
fn installed(output: &Output, tag: &str) {
    assert!(output.status.success(), "{}{}", stdout(output), stderr(output));
    assert!(stdout(output).contains(&format!("DEPLOY_OK {tag}")), "{}", stdout(output));
}

#[track_caller]
fn stopped(output: &Output) {
    assert!(!output.status.success(), "{}", stdout(output));
    assert!(!stdout(output).contains("DEPLOY_OK"), "{}", stdout(output));
}

// What happened on 2026-10-31, when v0.1.67 went out two minutes after v0.1.68.
#[test]
fn a_release_older_than_the_newest_installed_is_refused() {
    let server = Server::new();
    installed(&server.deploy("v0.1.68"), "v0.1.68");

    let refused = server.deploy("v0.1.67");
    stopped(&refused);
    assert!(stderr(&refused).contains("v0.1.67"), "{}", stderr(&refused));
    assert!(stderr(&refused).contains("v0.1.68"), "{}", stderr(&refused));
    assert!(stderr(&refused).contains("Roll back"), "{}", stderr(&refused));

    assert_eq!(server.binary(), "v0.1.68");
    assert_eq!(server.restarts(), 1);
}

#[test]
fn the_first_deploy_goes_in_with_nothing_to_compare_it_with() {
    let server = Server::new();
    installed(&server.deploy("v0.1.3"), "v0.1.3");
    assert_eq!(server.binary(), "v0.1.3");
}

// As text, v0.1.10 comes before v0.1.9.
#[test]
fn a_newer_release_goes_in_by_its_number() {
    let server = Server::new();
    installed(&server.deploy("v0.1.9"), "v0.1.9");
    installed(&server.deploy("v0.1.10"), "v0.1.10");
    assert_eq!(server.binary(), "v0.1.10");
    stopped(&server.deploy("v0.1.9"));
}

// A re-run of the run that's live, after its check of the site failed, say.
#[test]
fn the_same_release_can_go_in_again() {
    let server = Server::new();
    installed(&server.deploy("v0.1.68"), "v0.1.68");
    installed(&server.deploy("v0.1.68"), "v0.1.68");
}

#[test]
fn a_rollback_installs_an_older_release_on_purpose() {
    let server = Server::new();
    installed(&server.deploy("v0.1.68"), "v0.1.68");
    installed(&server.roll_back("v0.1.66"), "v0.1.66");
    assert_eq!(server.binary(), "v0.1.66");
}

#[test]
fn only_the_word_rollback_lets_an_older_release_in() {
    let server = Server::new();
    installed(&server.deploy("v0.1.68"), "v0.1.68");
    stopped(&server.run(&["v0.1.66", &sha256("v0.1.66"), "roll-back"]));
    assert_eq!(server.binary(), "v0.1.68");
}

#[test]
fn a_run_left_waiting_cant_undo_a_rollback() {
    let server = Server::new();
    installed(&server.deploy("v0.1.68"), "v0.1.68");
    installed(&server.roll_back("v0.1.66"), "v0.1.66");

    stopped(&server.deploy("v0.1.67"));
    assert_eq!(server.binary(), "v0.1.66");

    installed(&server.deploy("v0.1.69"), "v0.1.69");
}

#[test]
fn a_release_that_doesnt_come_up_doesnt_count_as_installed() {
    let server = Server::new();
    installed(&server.deploy("v0.1.68"), "v0.1.68");

    server.set_down(true);
    stopped(&server.deploy("v0.1.70"));
    assert_eq!(server.binary(), "v0.1.68");
    server.set_down(false);

    installed(&server.deploy("v0.1.69"), "v0.1.69");
}

#[test]
fn a_binary_with_the_wrong_hash_stays_out() {
    let server = Server::new();
    installed(&server.deploy("v0.1.68"), "v0.1.68");
    stopped(&server.run(&["v0.1.69", &sha256("something else")]));
    assert_eq!(server.binary(), "v0.1.68");
}

// Two approved runs both have to see what the other installed.
#[test]
fn one_deploy_runs_at_a_time() {
    let server = Server::new();
    let running = File::create(server.dir.join("lock")).unwrap();
    running.try_lock().unwrap();

    let second = server.deploy("v0.1.68");
    stopped(&second);
    assert!(stderr(&second).contains("Another deploy"), "{}", stderr(&second));
    assert_eq!(server.binary(), "the binary it started with");

    running.unlock().unwrap();
    installed(&server.deploy("v0.1.68"), "v0.1.68");
}
