//! L22 to L30: the owner's tasks. The doorbell hands the owner's own `task:` messages, and nothing
//! else from a room, to a separate worker that never reads the room. Everything here is plain
//! functions, so the rules can be tested without a server.

use std::{path::Path, process::Stdio, time::Duration};

use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    process::Command,
};

/// L26: longer than this, a task isn't run.
pub const MAX_TASK_CHARS: usize = 2000;
/// L27: tasks that may start on one UTC day.
pub const MAX_TASKS_PER_DAY: u32 = 5;
/// More than this much of a worker's output is dropped. Only its last line is ever used.
const MAX_TASK_OUTPUT: u64 = 1 << 20;
/// After the worker exits, how long to wait for its output to finish arriving.
const OUTPUT_GRACE: Duration = Duration::from_secs(5);

/// A task from the bot's owner. Its text came from a room, so it goes to the worker's stdin and
/// nowhere else.
#[derive(PartialEq)]
pub struct Task {
    pub hangout_id: String,
    pub message_id: i64,
    /// Empty when `too_long`.
    pub text: String,
    /// L26.
    pub too_long: bool,
    /// The owner's name if the server gave it and it is plain enough to put after an @.
    pub owner_name: Option<String>,
}

/// The doorbell logs what it rings for, and a task's words aren't for logs.
impl std::fmt::Debug for Task {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Task")
            .field("hangout_id", &self.hangout_id)
            .field("message_id", &self.message_id)
            .field("text_chars", &self.text.chars().count())
            .field("too_long", &self.too_long)
            .finish()
    }
}

impl Task {
    pub(super) fn key(&self) -> String {
        format!("{}/{}", self.hangout_id, self.message_id)
    }

    /// L28: the worker's `TASK_ID`.
    pub fn id(&self) -> String {
        task_id(&self.hangout_id, self.message_id)
    }
}

/// What a message said after `@name task:`.
#[derive(Debug, PartialEq)]
pub enum TaskText {
    Text(String),
    /// L26.
    TooLong,
}

/// A name safe to put after an @ in a message the doorbell writes: words, spaces and a few marks.
pub(super) fn plain_name(name: &str) -> bool {
    !name.is_empty()
        && name.chars().count() <= 40
        && !name.starts_with(' ')
        && !name.ends_with(' ')
        && name.chars().all(|c| c.is_alphanumeric() || " _-.'".contains(c))
}

/// L22: `@name task: text`, at the start of the message, with `@name` a whole word. Says nothing
/// about who sent it.
pub fn parse_task(body: &str, name: &str) -> Option<TaskText> {
    let is_word = |c: char| c.is_alphanumeric() || c == '_';
    let mut rest = body.trim().strip_prefix('@')?;
    for expected in name.chars() {
        let mut chars = rest.chars();
        let found = chars.next()?;
        if !found.to_lowercase().eq(expected.to_lowercase()) {
            return None;
        }
        rest = chars.as_str();
    }
    if rest.starts_with(is_word) {
        return None;
    }
    let rest = rest.trim_start();
    let head = rest.get(.."task:".len())?;
    if !head.eq_ignore_ascii_case("task:") {
        return None;
    }
    let text = rest["task:".len()..].trim();
    match text {
        "" => None,
        text if text.chars().count() > MAX_TASK_CHARS => Some(TaskText::TooLong),
        text => Some(TaskText::Text(text.to_string())),
    }
}

/// L28: `TASK_ID`, from ids the server made, with anything but letters, numbers, `-` and `_`
/// swapped for `-`.
pub fn task_id(hangout_id: &str, message_id: i64) -> String {
    format!("{hangout_id}-{message_id}")
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() || c == '-' || c == '_' { c } else { '-' })
        .take(100)
        .collect()
}

/// L29: the last non-empty line of a worker's output, if it is a GitHub pull request address and
/// nothing else. Anything else the worker prints is thrown away.
pub fn pr_url(output: &str) -> Option<String> {
    let line = output.lines().map(str::trim).rfind(|line| !line.is_empty())?;
    let mut parts = line.strip_prefix("https://github.com/")?.split('/');
    let (owner, repo, pull, number) = (parts.next()?, parts.next()?, parts.next()?, parts.next()?);
    let owner_ok = (1..=39).contains(&owner.len())
        && owner.chars().all(|c| c.is_ascii_alphanumeric() || c == '-');
    let repo_ok = (1..=100).contains(&repo.len())
        && repo != "."
        && repo != ".."
        && repo.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_' || c == '.');
    let number_ok = (1..=10).contains(&number.len()) && number.chars().all(|c| c.is_ascii_digit());
    (parts.next().is_none() && pull == "pull" && owner_ok && repo_ok && number_ok)
        .then(|| line.to_string())
}

/// What the doorbell tells a room about a task.
#[derive(Debug)]
pub enum TaskNote<'a> {
    Started,
    Busy,
    Limit,
    TooLong,
    /// The worker's whole output, of which only a pull request address is ever used.
    Finished(&'a str),
    Failed,
    TimedOut,
}

/// L29: fixed words, and an @ for the owner only when their name is plain.
pub fn task_note(owner: Option<&str>, message_id: i64, note: &TaskNote) -> String {
    let n = message_id;
    let words = match note {
        TaskNote::Started => format!("Started the task from message {n}."),
        TaskNote::Busy => format!("A task is already running, so I didn't start message {n}."),
        TaskNote::Limit => format!(
            "I've hit today's limit of {MAX_TASKS_PER_DAY} tasks, so I didn't start message {n}."
        ),
        TaskNote::TooLong => format!(
            "The task in message {n} is longer than {MAX_TASK_CHARS} characters, so I didn't \
             start it."
        ),
        TaskNote::Finished(output) => match pr_url(output) {
            Some(url) => format!("Finished the task from message {n}: {url}"),
            None => format!("The task from message {n} finished without a pull request."),
        },
        TaskNote::Failed => format!("The task from message {n} failed."),
        TaskNote::TimedOut => format!("The task from message {n} timed out."),
    };
    match owner {
        Some(name) if plain_name(name) => format!("@{name} {words}"),
        _ => words,
    }
}

/// How a worker ended.
#[derive(Debug, PartialEq)]
pub enum TaskOutcome {
    /// It exited cleanly, with this on stdout.
    Done(String),
    /// It couldn't start or exited with an error. The reason is for the doorbell's log, not a room.
    Failed(String),
    TimedOut,
}

/// L27 and L28: runs `program` with the task on stdin. It gets `TASK_ID` and never this bot's
/// token or the server's address, even if the doorbell has them. Its stdout is captured, and it is
/// killed with whatever it started if it runs past `timeout`.
pub async fn run_task(program: &Path, id: &str, text: &str, timeout: Duration) -> TaskOutcome {
    let mut command = Command::new(program);
    command
        .env("TASK_ID", id)
        .env_remove("AGENTHANGOUT_TOKEN")
        .env_remove("AGENTHANGOUT_URL")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .kill_on_drop(true);
    #[cfg(unix)]
    command.process_group(0);
    let mut child = match command.spawn() {
        Ok(child) => child,
        Err(error) => return TaskOutcome::Failed(error.to_string()),
    };
    let pid = child.id();
    let (Some(mut stdin), Some(stdout)) = (child.stdin.take(), child.stdout.take()) else {
        return TaskOutcome::Failed("the worker has no stdin or stdout".to_string());
    };
    // Writing can block if the worker never reads, so it runs beside the wait.
    let text = text.as_bytes().to_vec();
    tokio::spawn(async move {
        let _ = stdin.write_all(&text).await;
    });
    let reader = tokio::spawn(async move {
        let mut output = Vec::new();
        let _ = stdout.take(MAX_TASK_OUTPUT).read_to_end(&mut output).await;
        output
    });
    match tokio::time::timeout(timeout, child.wait()).await {
        Ok(Ok(status)) if status.success() => {
            let output = tokio::time::timeout(OUTPUT_GRACE, reader).await;
            TaskOutcome::Done(
                String::from_utf8_lossy(&output.unwrap_or(Ok(Vec::new())).unwrap_or_default())
                    .into(),
            )
        }
        Ok(Ok(status)) => TaskOutcome::Failed(status.to_string()),
        Ok(Err(error)) => TaskOutcome::Failed(error.to_string()),
        Err(_) => {
            kill_tree(pid).await;
            let _ = child.kill().await;
            reader.abort();
            TaskOutcome::TimedOut
        }
    }
}

/// Ends a worker and everything it started, which killing only the worker would leave running.
async fn kill_tree(pid: Option<u32>) {
    let Some(pid) = pid else { return };
    let pid = pid.to_string();
    let _ = if cfg!(windows) {
        Command::new("taskkill").args(["/PID", &pid, "/T", "/F"]).output().await
    } else {
        Command::new("kill").args(["-KILL", &format!("-{pid}")]).output().await
    };
}

#[cfg(test)]
mod tests {
    use super::*;

    fn text(body: &str, name: &str) -> Option<String> {
        match parse_task(body, name)? {
            TaskText::Text(text) => Some(text),
            TaskText::TooLong => Some("<too long>".to_string()),
        }
    }

    // L22
    #[test]
    fn a_task_starts_with_the_bot_and_task() {
        assert_eq!(text("@Claude task: fix the typo", "Claude").as_deref(), Some("fix the typo"));
        assert_eq!(text("  @claude TASK:fix it  ", "Claude").as_deref(), Some("fix it"));
        assert_eq!(text("@Claude   task:\n\ndo a\nb", "Claude").as_deref(), Some("do a\nb"));
        assert_eq!(text("@Cake Claude task: x", "Cake Claude").as_deref(), Some("x"));
        assert_eq!(text("@cake CLAUDE Task: x", "Cake Claude").as_deref(), Some("x"));
    }

    // L22
    #[test]
    fn anything_else_is_an_ordinary_mention() {
        assert_eq!(text("hey @Claude task: fix it", "Claude"), None, "in the middle");
        assert_eq!(text("Claude task: fix it", "Claude"), None, "no @");
        assert_eq!(text("@Claude: task: fix it", "Claude"), None, "punctuation");
        assert_eq!(text("@Claude fix it", "Claude"), None, "no task:");
        assert_eq!(text("@Claude task fix it", "Claude"), None, "no colon");
        assert_eq!(text("@Claude tasks: fix it", "Claude"), None);
        assert_eq!(text("@Claudette task: fix it", "Claude"), None, "another name");
        assert_eq!(text("@Claudetask: fix it", "Claude"), None, "not a whole word");
        assert_eq!(text("@Cake task: fix it", "Cake Claude"), None, "half a name");
        assert_eq!(text("@Cake Claude task: fix it", "Cake"), None, "a shorter bot");
        assert_eq!(text("@Claude task:", "Claude"), None, "empty");
        assert_eq!(text("@Claude task:   \n  ", "Claude"), None, "blank");
        assert_eq!(text("", "Claude"), None);
    }

    // L22
    #[test]
    fn odd_text_does_not_trip_the_parser() {
        assert_eq!(text("@Claude tésk: x", "Claude"), None);
        assert_eq!(text("@Claude tas\u{212a}: x", "Claude"), None);
        assert_eq!(text("@İsta task: x", "İsta").as_deref(), Some("x"));
        assert_eq!(text("@", "Claude"), None);
        assert_eq!(text("@Cla", "Claude"), None);
    }

    // L26
    #[test]
    fn a_task_can_be_2000_characters_and_no_more() {
        let body = |n: usize| format!("@Claude task: {}", "é".repeat(n));
        assert_eq!(text(&body(MAX_TASK_CHARS), "Claude").unwrap().chars().count(), MAX_TASK_CHARS);
        assert_eq!(parse_task(&body(MAX_TASK_CHARS + 1), "Claude"), Some(TaskText::TooLong));
    }

    // L29
    #[test]
    fn only_a_pull_request_address_comes_back_from_a_worker() {
        let url = "https://github.com/CakeRepository/agentshangout/pull/166";
        assert_eq!(pr_url(url).as_deref(), Some(url));
        assert_eq!(pr_url(&format!("building...\r\n{url}\r\n\r\n")).as_deref(), Some(url));
        assert_eq!(pr_url(&format!("{url}  \n")).as_deref(), Some(url));
        assert_eq!(
            pr_url("https://github.com/a/b.c_d/pull/1").as_deref(),
            Some("https://github.com/a/b.c_d/pull/1")
        );

        for bad in [
            "",
            "FAILED",
            "\n\n",
            &format!("{url} and more"),
            &format!("see {url}"),
            &format!("{url}/files"),
            &format!("{url}/"),
            &format!("{url}?x=1"),
            &format!("{url}#issue"),
            &format!("{url}\nFAILED"),
            "https://github.com/a/b/pull/",
            "https://github.com/a/b/pull/12a",
            "https://github.com/a/b/pull/12345678901",
            "https://github.com/a/b/pulls/1",
            "https://github.com/a/b/issues/1",
            "https://github.com/a/pull/1",
            "https://github.com/a/../pull/1",
            "https://github.com/-/./pull/1",
            "https://github.com/a b/c/pull/1",
            "https://github.com/a/b c/pull/1",
            "http://github.com/a/b/pull/1",
            "https://gist.github.com/a/b/pull/1",
            "https://github.com.evil.example/a/b/pull/1",
            "https://evil.example/https://github.com/a/b/pull/1",
            "https://user@github.com/a/b/pull/1",
            "https://github.com/a/b/pull/1)[x](https://evil.example)",
            "https://github.com/a/b/pull/1\u{200b}",
            "https://github.com/a/b%2Fc/pull/1",
            "https://github.com/@room/b/pull/1",
            "https://github.com/a/b/pull/١",
        ] {
            assert_eq!(pr_url(bad), None, "{bad:?}");
        }
    }

    // L29
    #[test]
    fn the_room_hears_fixed_words() {
        let say = |owner, note| task_note(owner, 7, &note);
        assert_eq!(say(None, TaskNote::Started), "Started the task from message 7.");
        assert_eq!(
            say(Some("Justin"), TaskNote::Busy),
            "@Justin A task is already running, so I didn't start message 7."
        );
        assert_eq!(
            say(None, TaskNote::Limit),
            "I've hit today's limit of 5 tasks, so I didn't start message 7."
        );
        assert_eq!(
            say(None, TaskNote::TooLong),
            "The task in message 7 is longer than 2000 characters, so I didn't start it."
        );
        assert_eq!(say(None, TaskNote::Failed), "The task from message 7 failed.");
        assert_eq!(say(None, TaskNote::TimedOut), "The task from message 7 timed out.");
        let url = "https://github.com/a/b/pull/9";
        let output = format!("secret: abc\n{url}\n");
        assert_eq!(
            say(Some("Cake Claude"), TaskNote::Finished(&output)),
            format!("@Cake Claude Finished the task from message 7: {url}")
        );
        let nothing = say(None, TaskNote::Finished("token=hunter2\nall done"));
        assert_eq!(nothing, "The task from message 7 finished without a pull request.");
        // A name that could be anything else isn't put in a message.
        for odd in ["", "Ju\nstin", "Justin @everyone!", " Justin", "<b>x</b>"] {
            assert_eq!(say(Some(odd), TaskNote::Started), "Started the task from message 7.");
        }
    }

    // L28
    #[test]
    fn the_task_id_is_made_of_safe_characters() {
        assert_eq!(task_id("abc-123", 7), "abc-123-7");
        assert_eq!(task_id("a/b", 1), "a-b-1");
        let odd = task_id("a/b c;d$(x)\"..\\`|&<>", 3);
        assert!(odd.chars().all(|c| c.is_ascii_alphanumeric() || c == '-'), "{odd}");
        assert!(task_id(&"x".repeat(500), 1).len() <= 100);
        assert!(task_id("é", 1).chars().all(|c| c.is_ascii_alphanumeric() || c == '-'));
    }
}
