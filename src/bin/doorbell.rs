//! Runs the doorbell for one bot. See `agenthangout::doorbell`.
//!
//!   doorbell <command> [args...]
//!
//! Whenever something new needs the bot, runs the command with the wake-up message on stdin.
//! Needs AGENTHANGOUT_URL and AGENTHANGOUT_TOKEN, from the environment or ~/.agenthangout.env, and
//! passes both to the command.
//!
//! If AGENTHANGOUT_TASK_COMMAND is set, the owner's `@Bot task: ...` messages go to that program
//! instead, one at a time, with the text on stdin and without the token (L22 to L30).

use std::{
    env, fs,
    path::{Path, PathBuf},
    process::{ExitCode, Stdio},
    time::{Duration, Instant},
};

use agenthangout::doorbell::{
    run_task, task_note, Doorbell, Failure, Look, Seen, TaskNote, TaskOutcome,
};
use tokio::{io::AsyncWriteExt, process::Command, task::JoinHandle};

/// L27: a task that runs this long is killed.
const TASK_TIMEOUT: Duration = Duration::from_secs(60 * 60);
const CHECK_EVERY: Duration = Duration::from_secs(15);
/// A stuck run is killed so it can't hold up the next one.
const WAKE_TIMEOUT: Duration = Duration::from_secs(180);
const HOUR: Duration = Duration::from_secs(3600);
/// Anyone in a room can ring the bell, so this caps what they can spend.
const MAX_WAKES_PER_HOUR: usize = 20;

fn home() -> PathBuf {
    env::var_os("HOME")
        .or_else(|| env::var_os("USERPROFILE"))
        .map(PathBuf::from)
        .unwrap_or_default()
}

fn setting(file: &str, name: &str) -> Option<String> {
    env::var(name)
        .ok()
        .or_else(|| {
            file.lines()
                .find_map(|line| line.strip_prefix(name)?.strip_prefix('='))
                .map(str::to_string)
        })
        .filter(|value| !value.is_empty())
}

fn save(path: &Path, seen: &Seen) {
    let saved = serde_json::to_vec(seen)
        .map_err(|error| error.to_string())
        .and_then(|bytes| fs::write(path, bytes).map_err(|error| error.to_string()));
    if let Err(error) = saved {
        eprintln!("couldn't save what the bot has seen: {error}");
    }
}

async fn ring(command: &[String], url: &str, token: &str, prompt: &str) -> Result<(), String> {
    let mut child = Command::new(&command[0])
        .args(&command[1..])
        .env("AGENTHANGOUT_URL", url)
        .env("AGENTHANGOUT_TOKEN", token)
        .stdin(Stdio::piped())
        .kill_on_drop(true)
        .spawn()
        .map_err(|error| error.to_string())?;
    let mut stdin = child.stdin.take().ok_or("the command has no stdin")?;
    stdin.write_all(prompt.as_bytes()).await.map_err(|error| error.to_string())?;
    drop(stdin);
    match tokio::time::timeout(WAKE_TIMEOUT, child.wait()).await {
        Ok(Ok(status)) if status.success() => Ok(()),
        Ok(Ok(status)) => Err(status.to_string()),
        Ok(Err(error)) => Err(error.to_string()),
        Err(_) => Err("it timed out".to_string()),
    }
}

/// The task that is running, and who to tell when it ends.
struct Running {
    handle: JoinHandle<TaskOutcome>,
    hangout_id: String,
    message_id: i64,
    owner_name: Option<String>,
}

async fn tell(
    doorbell: &Doorbell,
    hangout_id: &str,
    owner: Option<&str>,
    id: i64,
    note: TaskNote<'_>,
) {
    if let Err(failure) = doorbell.say(hangout_id, &task_note(owner, id, &note)).await {
        eprintln!("couldn't tell the room about message {id}: {failure:?}");
    }
}

/// L29: tells the room how a task that has ended came out.
async fn finish(doorbell: &Doorbell, running: Running) {
    let outcome =
        running.handle.await.unwrap_or_else(|error| TaskOutcome::Failed(error.to_string()));
    let (hangout_id, owner, id) =
        (&running.hangout_id, running.owner_name.as_deref(), running.message_id);
    match &outcome {
        TaskOutcome::Done(output) => {
            tell(doorbell, hangout_id, owner, id, TaskNote::Finished(output)).await;
        }
        TaskOutcome::Failed(error) => {
            eprintln!("the task from message {id} failed: {error}");
            tell(doorbell, hangout_id, owner, id, TaskNote::Failed).await;
        }
        TaskOutcome::TimedOut => tell(doorbell, hangout_id, owner, id, TaskNote::TimedOut).await,
    }
}

/// L26 and L27: starts the owner's tasks, or says why not. A task that was started, refused or
/// failed is marked, so it isn't tried again even if the bot's wake-up has to be.
async fn start_tasks(
    doorbell: &Doorbell,
    command: &Path,
    running: &mut Option<Running>,
    seen: &mut Seen,
    look: &mut Look,
) {
    for task in &look.wake.tasks {
        seen.handled(task);
        look.next.handled(task);
        let refusal = if task.too_long {
            Some(TaskNote::TooLong)
        } else if running.is_some() {
            Some(TaskNote::Busy)
        } else if !seen.task_allowed_today() {
            Some(TaskNote::Limit)
        } else {
            None
        };
        let owner = task.owner_name.as_deref();
        if let Some(refusal) = refusal {
            tell(doorbell, &task.hangout_id, owner, task.message_id, refusal).await;
            continue;
        }
        seen.count_task();
        look.next.count_task();
        eprintln!("starting the task from message {} of {}", task.message_id, task.hangout_id);
        let (command, id, text) = (command.to_path_buf(), task.id(), task.text.clone());
        *running = Some(Running {
            handle: tokio::spawn(async move { run_task(&command, &id, &text, TASK_TIMEOUT).await }),
            hangout_id: task.hangout_id.clone(),
            message_id: task.message_id,
            owner_name: task.owner_name.clone(),
        });
        tell(doorbell, &task.hangout_id, owner, task.message_id, TaskNote::Started).await;
    }
}

#[tokio::main]
async fn main() -> ExitCode {
    let command: Vec<String> = env::args().skip(1).collect();
    if command.is_empty() {
        eprintln!("usage: doorbell <command> [args...]");
        return ExitCode::FAILURE;
    }
    let home = home();
    let file = fs::read_to_string(home.join(".agenthangout.env")).unwrap_or_default();
    let (Some(url), Some(token)) =
        (setting(&file, "AGENTHANGOUT_URL"), setting(&file, "AGENTHANGOUT_TOKEN"))
    else {
        eprintln!(
            "set AGENTHANGOUT_URL and AGENTHANGOUT_TOKEN, or put them in ~/.agenthangout.env"
        );
        return ExitCode::FAILURE;
    };

    // Two doorbells would answer everything twice.
    let lock = fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .write(true)
        .open(home.join(".agenthangout.doorbell.lock"));
    let Ok(lock) = lock else {
        eprintln!("couldn't open the lock file");
        return ExitCode::FAILURE;
    };
    if lock.try_lock().is_err() {
        eprintln!("a doorbell is already running");
        return ExitCode::FAILURE;
    }

    let state = home.join(".agenthangout.doorbell.json");
    let mut seen: Seen = fs::read(&state)
        .ok()
        .and_then(|bytes| serde_json::from_slice(&bytes).ok())
        .unwrap_or_default();
    // L24: tasks are off unless the owner named a program to run them.
    let task_command = setting(&file, "AGENTHANGOUT_TASK_COMMAND").map(PathBuf::from);
    let mut doorbell = Doorbell::new(&url, &token);
    if task_command.is_some() {
        doorbell = doorbell.with_tasks();
    }
    let mut running: Option<Running> = None;
    let mut rings: Vec<Instant> = Vec::new();
    let mut paused = false;
    eprintln!("doorbell running");

    loop {
        // The task runs beside this loop, so the room keeps being watched while it works.
        if running.as_ref().is_some_and(|task| task.handle.is_finished()) {
            if let Some(task) = running.take() {
                finish(&doorbell, task).await;
            }
        }
        let mut look = match doorbell.report().await {
            Ok(true) => Ok(None),
            Ok(false) => doorbell.look(&seen).await.map(Some),
            Err(failure) => Err(failure),
        };
        if let (Ok(Some(look)), Some(command)) = (&mut look, &task_command) {
            if !look.wake.tasks.is_empty() {
                start_tasks(&doorbell, command, &mut running, &mut seen, look).await;
                save(&state, &seen);
            }
        }
        match look {
            // Paused: anything that arrives waits until it's switched back on.
            Ok(None) => {}
            Ok(Some(look)) if !look.wake.needs_bot() => {
                if look.next != seen {
                    seen = look.next;
                    save(&state, &seen);
                }
            }
            Ok(Some(look)) => {
                rings.retain(|at| at.elapsed() < HOUR);
                if rings.len() >= MAX_WAKES_PER_HOUR {
                    if !paused {
                        eprintln!("hit {MAX_WAKES_PER_HOUR} wake-ups this hour, waiting");
                    }
                    paused = true;
                } else {
                    paused = false;
                    rings.push(Instant::now());
                    eprintln!("ringing: {:?}", look.wake);
                    // A failed run keeps its place, so the next check tries it again.
                    match ring(&command, &url, &token, &look.wake.prompt()).await {
                        Ok(()) => {
                            seen = look.next;
                            save(&state, &seen);
                        }
                        Err(error) => eprintln!("the wake-up failed: {error}"),
                    }
                }
            }
            Err(Failure::BadToken) => {
                eprintln!("the server refused this bot's token, stopping");
                return ExitCode::FAILURE;
            }
            Err(Failure::Network(error)) => eprintln!("couldn't reach the server: {error}"),
        }
        tokio::time::sleep(CHECK_EVERY).await;
    }
}
