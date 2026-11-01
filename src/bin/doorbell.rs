//! Runs the doorbell for one bot. See `agenthangout::doorbell`.
//!
//!   doorbell <command> [args...]
//!
//! Whenever something new needs the bot, runs the command with the wake-up message on stdin.
//! Needs AGENTHANGOUT_URL and AGENTHANGOUT_TOKEN, from the environment or ~/.agenthangout.env, and
//! passes both to the command.
//!
//! If AGENTHANGOUT_TASK_COMMAND is set, the owner's `@Bot task: ...` messages go to that program
//! instead, one at a time, with the text on stdin and without the token (OT1 to OT9).

use std::{
    env, fs,
    path::{Path, PathBuf},
    process::{ExitCode, Stdio},
    time::{Duration, Instant},
};

use agenthangout::doorbell::{
    refusal, run_task, task_note, valid_repo, Doorbell, Failure, Look, Seen, TaskNote, TaskOutcome,
};
use tokio::{io::AsyncWriteExt, process::Command, task::JoinHandle};

/// OT6: a task that runs this long is killed.
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

fn save(path: &Path, seen: &Seen) -> Result<(), String> {
    use std::io::Write;
    let bytes = serde_json::to_vec(seen).map_err(|e| e.to_string())?;
    let pending = path.with_extension("json.pending");
    // A process lock protects this path. Never truncate the last valid checkpoint.
    let result = (|| -> std::io::Result<()> {
        let mut file = fs::OpenOptions::new().write(true).create_new(true).open(&pending)?;
        file.write_all(&bytes)?;
        file.sync_all()?;
        drop(file);
        fs::rename(&pending, path)?;
        #[cfg(unix)]
        fs::File::open(path.parent().unwrap_or(Path::new(".")))?.sync_all()?;
        Ok(())
    })();
    result.map_err(|e| format!("couldn't durably save doorbell state: {e}"))
}

fn load(path: &Path) -> Result<Seen, String> {
    // A leftover pending write means the prior reservation had an uncertain outcome.
    if path.with_extension("json.pending").exists() {
        return Err("unfinished state save; inspect the checkpoint before restarting".into());
    }
    match fs::read(path) {
        Ok(bytes) => {
            serde_json::from_slice(&bytes).map_err(|e| format!("invalid doorbell state: {e}"))
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Seen::default()),
        Err(e) => Err(format!("couldn't read doorbell state: {e}")),
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

/// The program that does tasks, and the repository its pull requests must be in (OT10).
struct Worker {
    command: PathBuf,
    repo: String,
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

/// OT8: tells the room how a task that has ended came out. It tries twice, since the owner has
/// nowhere else to find a pull request address.
async fn finish(doorbell: &Doorbell, worker: &Worker, running: Running) {
    let outcome =
        running.handle.await.unwrap_or_else(|error| TaskOutcome::Failed(error.to_string()));
    let (hangout_id, owner, id) =
        (&running.hangout_id, running.owner_name.as_deref(), running.message_id);
    let note = match &outcome {
        TaskOutcome::Done(output) => TaskNote::Finished(output, &worker.repo),
        TaskOutcome::Failed(error) => {
            eprintln!("the task from message {id} failed: {error}");
            TaskNote::Failed
        }
        TaskOutcome::TimedOut => TaskNote::TimedOut,
    };
    let text = task_note(owner, id, &note);
    for attempt in 1..=2 {
        match doorbell.say(hangout_id, &text).await {
            Ok(()) => return,
            Err(failure) => eprintln!("couldn't tell the room about message {id}: {failure:?}"),
        }
        if attempt == 1 {
            tokio::time::sleep(Duration::from_secs(5)).await;
        }
    }
    eprintln!("it was: {text}");
}

/// OT5 and OT6: starts the owner's tasks, or says why not. A task that was started, refused or
/// failed is marked and saved before the room hears anything, so it isn't tried again even if the
/// bot's wake-up has to be, or the doorbell stops.
async fn start_tasks(
    doorbell: &Doorbell,
    worker: &Worker,
    running: &mut Option<Running>,
    seen: &mut Seen,
    look: &mut Look,
    state: &Path,
) -> Result<(), String> {
    for task in &look.wake.tasks {
        seen.handled(task);
        look.next.handled(task);
        let refusal = refusal(task, running.is_some(), seen);
        let owner = task.owner_name.as_deref();
        if let Some(refusal) = refusal {
            save(state, seen)?;
            tell(doorbell, &task.hangout_id, owner, task.message_id, refusal).await;
            continue;
        }
        seen.count_task();
        look.next.count_task();
        save(state, seen)?;
        eprintln!("starting the task from message {} of {}", task.message_id, task.hangout_id);
        let (command, id, text) = (worker.command.clone(), task.id(), task.text.clone());
        let repo = worker.repo.clone();
        *running = Some(Running {
            handle: tokio::spawn(async move {
                run_task(&command, &id, &text, TASK_TIMEOUT, &repo).await
            }),
            hangout_id: task.hangout_id.clone(),
            message_id: task.message_id,
            owner_name: task.owner_name.clone(),
        });
        tell(doorbell, &task.hangout_id, owner, task.message_id, TaskNote::Started).await;
    }
    Ok(())
}

/// OT3 and OT10: the worker, if the owner named one. Naming one without the repository its pull
/// requests belong to is a mistake, so the doorbell doesn't start.
fn worker(file: &str) -> Result<Option<Worker>, String> {
    let Some(command) = setting(file, "AGENTHANGOUT_TASK_COMMAND") else { return Ok(None) };
    let repo = setting(file, "AGENTHANGOUT_TASK_REPO")
        .ok_or("AGENTHANGOUT_TASK_COMMAND needs AGENTHANGOUT_TASK_REPO, as owner/name")?;
    if !valid_repo(&repo) {
        return Err(format!("AGENTHANGOUT_TASK_REPO should look like owner/name, not {repo}"));
    }
    Ok(Some(Worker { command: PathBuf::from(command), repo }))
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
    let mut seen = match load(&state) {
        Ok(seen) => seen,
        Err(error) => {
            eprintln!("{error}");
            return ExitCode::FAILURE;
        }
    };
    // OT3: tasks are off unless the owner named a program to run them.
    let worker = match worker(&file) {
        Ok(worker) => worker,
        Err(error) => {
            eprintln!("{error}");
            return ExitCode::FAILURE;
        }
    };
    let mut doorbell = Doorbell::new(&url, &token);
    if worker.is_some() {
        doorbell = doorbell.with_tasks();
    }
    let mut running: Option<Running> = None;
    let mut rings: Vec<Instant> = Vec::new();
    let mut paused = false;
    eprintln!("doorbell running");

    loop {
        // The task runs beside this loop, so the room keeps being watched while it works.
        let mut look = match doorbell.report().await {
            Ok(true) => Ok(None),
            Ok(false) => doorbell.look(&seen).await.map(Some),
            Err(failure) => Err(failure),
        };
        // That took a moment, and the worker may have ended in it: a task that arrived just as it
        // did mustn't be turned away as busy.
        if let Some(worker) = &worker {
            if running.as_ref().is_some_and(|task| task.handle.is_finished()) {
                if let Some(task) = running.take() {
                    finish(&doorbell, worker, task).await;
                }
            }
            if let Ok(Some(look)) = &mut look {
                if !look.wake.tasks.is_empty() {
                    if let Err(error) =
                        start_tasks(&doorbell, worker, &mut running, &mut seen, look, &state).await
                    {
                        eprintln!("{error}; stopping before another task can start");
                        return ExitCode::FAILURE;
                    }
                }
            }
        }
        match look {
            // Paused: anything that arrives waits until it's switched back on.
            Ok(None) => {}
            Ok(Some(look)) if !look.wake.needs_bot() => {
                if look.next != seen {
                    seen = look.next;
                    if let Err(error) = save(&state, &seen) {
                        eprintln!("{error}");
                        return ExitCode::FAILURE;
                    }
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
                            if let Err(error) = save(&state, &seen) {
                                eprintln!("{error}");
                                return ExitCode::FAILURE;
                            }
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

#[cfg(test)]
mod tests {
    use super::*;
    use agenthangout::doorbell::{Task, Wake};

    #[tokio::test]
    async fn an_unwritable_reservation_never_starts_a_worker() {
        let dir = env::temp_dir().join(format!("hangout-reservation-{}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();
        let doorbell = Doorbell::new("http://127.0.0.1:1", "local-test-only");
        let worker = Worker { command: dir.join("never-start"), repo: "a/b".into() };
        let mut running = None;
        let mut seen = Seen::default();
        let mut look = Look {
            next: seen.clone(),
            wake: Wake {
                tasks: vec![Task {
                    hangout_id: "room".into(),
                    message_id: 1,
                    text: "test".into(),
                    too_long: false,
                    owner_name: None,
                }],
                ..Wake::default()
            },
        };
        let _ = start_tasks(&doorbell, &worker, &mut running, &mut seen, &mut look, &dir).await;
        assert!(running.is_none(), "must persist the reservation before launching any worker");
        let _ = fs::remove_file(dir.with_extension("json.pending"));
        fs::remove_dir(&dir).unwrap();
    }
    #[test]
    fn checkpoints_replace_atomically_and_corruption_never_resets_the_budget() {
        let dir = env::temp_dir().join(format!("hangout-checkpoint-{}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();
        let path = dir.join("state.json");
        let mut seen = Seen::default();
        save(&path, &seen).unwrap();
        seen.count_task();
        save(&path, &seen).unwrap();
        assert_eq!(load(&path).unwrap(), seen);
        let original = fs::read(&path).unwrap();
        let pending = path.with_extension("json.pending");
        fs::write(&pending, b"unfinished").unwrap();
        assert!(save(&path, &Seen::default()).is_err());
        assert_eq!(fs::read(&path).unwrap(), original);
        assert!(load(&path).is_err());
        fs::remove_file(pending).unwrap();
        fs::write(&path, b"{truncated").unwrap();
        assert!(load(&path).is_err());
        fs::remove_file(path).unwrap();
        fs::remove_dir(dir).unwrap();
    }
}
