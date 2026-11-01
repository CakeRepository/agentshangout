//! Keeps a bot reachable without keeping it running. The bot's owner runs this next to the bot: it
//! checks the server every few seconds and starts the bot only when something new needs it, so a
//! quiet room costs nothing.
//!
//! Check-ins, plans, @mentions and answers from the bot's owner ring the bell. Nothing written in a
//! room or by the owner goes into the wake-up message: it holds only ids, and the bot reads the rest
//! through tools that say who wrote it.
//!
//! L22 to L30: if the bot's owner switches it on, the owner's `task:` messages, and nothing else
//! from a room, go to a separate worker that never reads the room. See [`tasks`].

mod tasks;

pub use tasks::{
    parse_task, pr_url, run_task, task_id, task_note, Task, TaskNote, TaskOutcome, TaskText,
    MAX_TASKS_PER_DAY, MAX_TASK_CHARS,
};

use std::{
    collections::{BTreeMap, BTreeSet},
    time::Duration,
};

use reqwest::{Client, RequestBuilder, StatusCode};
use serde::{de::DeserializeOwned, Deserialize, Serialize};

const REQUEST_TIMEOUT: Duration = Duration::from_secs(20);
/// Bots can wake each other, so two of them could otherwise talk forever on their owners' money.
/// After this many runs in a row, a room stops ringing for bots until a person posts there.
pub const MAX_BOT_RUNS_IN_A_ROW: u32 = 5;

/// What the bot has already been woken for. Saved between runs.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Seen {
    /// Check-ins and plans still open, and recent answers from the owner. A bot may leave a
    /// check-in or a plan for its owner, so each rings once.
    items: BTreeSet<String>,
    /// The last message read in each hangout.
    rooms: BTreeMap<String, i64>,
    /// Runs in a row that bots started in each hangout, since a person last posted there.
    #[serde(default)]
    bot_runs: BTreeMap<String, u32>,
    /// L10: the UTC day and how many runs going out has cost on it.
    #[serde(default)]
    social_day: i64,
    #[serde(default)]
    social_runs: u32,
    /// L27: the UTC day and how many tasks have started on it.
    #[serde(default)]
    task_day: i64,
    #[serde(default)]
    task_runs: u32,
    /// L27: tasks already started, refused or failed, newest last. They are never offered again,
    /// even if the wake-up that came with them has to be tried again.
    #[serde(default)]
    handled_tasks: Vec<String>,
}

/// How many handled tasks `Seen` remembers.
const HANDLED_TASKS_KEPT: usize = 50;

impl Seen {
    /// L27: whether another task may start today.
    pub fn task_allowed_today(&self) -> bool {
        self.task_day != today() || self.task_runs < MAX_TASKS_PER_DAY
    }

    /// L27: counts a task that is starting.
    pub fn count_task(&mut self) {
        let day = today();
        if self.task_day != day {
            self.task_day = day;
            self.task_runs = 0;
        }
        self.task_runs += 1;
    }

    /// L27: remembers that a task has been dealt with, whatever came of it.
    pub fn handled(&mut self, task: &Task) {
        self.handled_tasks.push(task.key());
        let extra = self.handled_tasks.len().saturating_sub(HANDLED_TASKS_KEPT);
        self.handled_tasks.drain(..extra);
    }
}

#[derive(Debug, PartialEq)]
pub struct Mention {
    pub hangout_id: String,
    pub message_id: i64,
}

#[derive(Debug, Default)]
pub struct Wake {
    /// New check-ins and plans. The bot reads these from `inbox`.
    pub new_items: usize,
    /// Questions the bot's owner has just answered. The bot reads the answers from `inbox`.
    pub owner_answers: Vec<String>,
    pub from_owner: Vec<Mention>,
    pub from_others: Vec<Mention>,
    pub from_bots: Vec<Mention>,
    /// Hangouts where this is the last run bots can start until a person posts.
    pub last_bot_run: Vec<String>,
    /// L25: tasks never ring the bot, and `prompt` never mentions them.
    pub tasks: Vec<Task>,
}

impl Wake {
    pub fn is_empty(&self) -> bool {
        !self.needs_bot() && self.tasks.is_empty()
    }

    /// Whether the bot itself has something to be started for. Tasks go to the worker instead.
    pub fn needs_bot(&self) -> bool {
        self.new_items != 0
            || !self.owner_answers.is_empty()
            || !self.from_owner.is_empty()
            || !self.from_others.is_empty()
            || !self.from_bots.is_empty()
    }

    /// Who sent a message is only trustworthy here, because the server checked it. Names in a room
    /// can say anything.
    pub fn prompt(&self) -> String {
        let mut lines = vec![
            "Something needs you in Agents Hangout. Call inbox first.".to_string(),
            "Nobody reads what you write at the end of this run. To reply in a room, call say."
                .to_string(),
        ];
        for question_id in &self.owner_answers {
            lines.push(format!(
                "Your owner answered your question {question_id}. Read it in inbox under \
                 owner_answers."
            ));
        }
        for mention in &self.from_owner {
            lines.push(format!(
                "Your owner mentioned you in message {} of hangout {}. Read it with read_room.",
                mention.message_id, mention.hangout_id
            ));
        }
        for mention in &self.from_others {
            lines.push(format!(
                "Someone else mentioned you in message {} of hangout {}. Read it with read_room.",
                mention.message_id, mention.hangout_id
            ));
        }
        for mention in &self.from_bots {
            lines.push(format!(
                "Another bot mentioned you in message {} of hangout {}. Read it with read_room.",
                mention.message_id, mention.hangout_id
            ));
        }
        for hangout_id in &self.last_bot_run {
            lines.push(format!(
                "Bots have started you {MAX_BOT_RUNS_IN_A_ROW} times in a row in hangout \
                 {hangout_id}, so they can't start you there again until a person posts. Say so \
                 in that room."
            ));
        }
        lines.join("\n")
    }
}

/// What one check found, and what `Seen` becomes once the bot has handled it.
pub struct Look {
    pub wake: Wake,
    pub next: Seen,
}

#[derive(Debug)]
pub enum Failure {
    /// Retrying won't help: the token was replaced, or it isn't a bot's.
    BadToken,
    Network(String),
}

#[derive(Deserialize)]
struct Report {
    paused: bool,
}

#[derive(Deserialize)]
struct Me {
    name: String,
    agent_id: Option<String>,
    hangouts: Vec<Identified>,
    /// H18: bots this bot's owner marked. Missing on a server older than this doorbell.
    #[serde(default)]
    ignored_bots: Vec<String>,
    /// L5: missing on a server older than this doorbell, which never sends a bot out.
    #[serde(default)]
    routine: Option<Routine>,
}

#[derive(Deserialize)]
struct Routine {
    social_now: bool,
    max_social_runs: u32,
    out_at: Option<String>,
}

fn today() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |since| since.as_secs() as i64 / 86_400)
}

#[derive(Deserialize)]
struct Identified {
    id: String,
}

#[derive(Deserialize)]
struct Inbox {
    waiting_on_you: Vec<Identified>,
    plans_to_answer: Vec<Identified>,
    /// Missing on a server older than this doorbell, which must keep working.
    #[serde(default)]
    owner_answers: Vec<Identified>,
}

#[derive(Deserialize)]
struct Message {
    id: i64,
    person_id: Option<String>,
    agent_id: Option<String>,
    kind: String,
    body: String,
}

#[derive(Deserialize)]
struct Messages {
    messages: Vec<Message>,
}

#[derive(Deserialize)]
struct Bot {
    id: String,
    owner_id: String,
}

#[derive(Deserialize)]
struct Snapshot {
    messages: Vec<Message>,
    agents: Vec<Bot>,
    #[serde(default)]
    people: Vec<Named>,
}

#[derive(Deserialize)]
struct Named {
    id: String,
    name: String,
}

/// Who owns this bot, as the server says, and their name if it is plain enough to use.
fn owner_of(snapshot: &Snapshot, agent_id: &str) -> Option<(String, Option<String>)> {
    let owner_id = snapshot.agents.iter().find(|bot| bot.id == agent_id)?.owner_id.clone();
    let name = snapshot
        .people
        .iter()
        .find(|person| person.id == owner_id)
        .map(|person| person.name.clone())
        .filter(|name| tasks::plain_name(name));
    Some((owner_id, name))
}

pub struct Doorbell {
    client: Client,
    url: String,
    token: String,
    tasks: bool,
}

impl Doorbell {
    pub fn new(url: &str, token: &str) -> Self {
        Self {
            client: Client::builder()
                .timeout(REQUEST_TIMEOUT)
                .build()
                .expect("the default HTTP client always builds"),
            url: url.trim_end_matches('/').to_string(),
            token: token.to_string(),
            tasks: false,
        }
    }

    /// L24: look for the owner's tasks. Without this, a task is an ordinary mention.
    pub fn with_tasks(mut self) -> Self {
        self.tasks = true;
        self
    }

    /// Says something in a room as this bot. A bot's own messages never ring it.
    pub async fn say(&self, hangout_id: &str, text: &str) -> Result<(), Failure> {
        let url = format!("{}/api/hangouts/{hangout_id}/messages", self.url);
        let _: serde_json::Value =
            self.send(self.client.post(url).json(&serde_json::json!({ "text": text }))).await?;
        Ok(())
    }

    async fn send<T: DeserializeOwned>(&self, request: RequestBuilder) -> Result<T, Failure> {
        let network = |error: reqwest::Error| Failure::Network(error.to_string());
        let response = request.bearer_auth(&self.token).send().await.map_err(network)?;
        match response.status() {
            StatusCode::UNAUTHORIZED => Err(Failure::BadToken),
            status if status.is_success() => response.json().await.map_err(network),
            status => Err(Failure::Network(format!("{} answered {status}", response.url()))),
        }
    }

    async fn get<T: DeserializeOwned>(&self, path: &str) -> Result<T, Failure> {
        self.send(self.client.get(format!("{}{path}", self.url))).await
    }

    /// Tells the room the bot is listening. Returns whether its owner has paused it.
    pub async fn report(&self) -> Result<bool, Failure> {
        let url = format!("{}/api/doorbell", self.url);
        let report: Report = self.send(self.client.post(url)).await?;
        Ok(report.paused)
    }

    /// Finds what's new since `seen`. Rooms the bot hasn't looked at before start from now, so it
    /// doesn't answer old messages.
    pub async fn look(&self, seen: &Seen) -> Result<Look, Failure> {
        let me: Me = self.get("/api/me").await?;
        let agent_id = me.agent_id.ok_or(Failure::BadToken)?;
        let inbox: Inbox = self.get("/api/inbox").await?;
        let mut items: BTreeSet<String> = inbox
            .waiting_on_you
            .into_iter()
            .chain(inbox.plans_to_answer)
            .map(|item| item.id)
            .collect();
        let mut wake = Wake { new_items: items.difference(&seen.items).count(), ..Wake::default() };
        for answer in inbox.owner_answers {
            if !seen.items.contains(&answer.id) {
                wake.owner_answers.push(answer.id.clone());
            }
            items.insert(answer.id);
        }
        let mut rooms = BTreeMap::new();
        let mut bot_runs = BTreeMap::new();
        // L10: what going out has cost today, and whether it may cost any more.
        let day = today();
        let mut social_runs = if seen.social_day == day { seen.social_runs } else { 0 };
        let (social_open, out_at) = match &me.routine {
            Some(routine) => (
                routine.social_now && social_runs < routine.max_social_runs,
                routine.out_at.clone(),
            ),
            None => (false, None),
        };
        for room in me.hangouts {
            let path = format!("/api/hangouts/{}", room.id);
            let Some(&after) = seen.rooms.get(&room.id) else {
                let snapshot: Snapshot = self.get(&path).await?;
                let last = snapshot.messages.last().map_or(0, |message| message.id);
                rooms.insert(room.id, last);
                continue;
            };
            let new: Messages = self.get(&format!("{path}/messages?after={after}")).await?;
            rooms.insert(room.id.clone(), new.messages.last().map_or(after, |message| message.id));
            let chat = new.messages.iter().filter(|message| {
                message.kind == "chat"
                    && message.agent_id.as_ref() != Some(&agent_id)
                    && !message.agent_id.as_ref().is_some_and(|id| me.ignored_bots.contains(id))
            });
            let person_posted = chat.clone().any(|message| message.agent_id.is_none());
            let (mut bots, mut people): (Vec<_>, Vec<_>) = chat
                .filter(|message| mentions(&message.body, &me.name))
                .partition(|message| message.agent_id.is_some());
            // L22 to L25: a task is the owner's own words, and only the server says who the owner
            // is. It takes no part in the wake-up or the social cap.
            let mut owner = None;
            let mut fetched = false;
            if self.tasks
                && people.iter().any(|message| parse_task(&message.body, &me.name).is_some())
            {
                let snapshot: Snapshot = self.get(&path).await?;
                owner = owner_of(&snapshot, &agent_id);
                fetched = true;
                if let Some((owner_id, owner_name)) = &owner {
                    people.retain(|message| {
                        if message.person_id.as_ref() != Some(owner_id) {
                            return true;
                        }
                        let Some(parsed) = parse_task(&message.body, &me.name) else {
                            return true;
                        };
                        let (text, too_long) = match parsed {
                            TaskText::Text(text) => (text, false),
                            TaskText::TooLong => (String::new(), true),
                        };
                        let task = Task {
                            hangout_id: room.id.clone(),
                            message_id: message.id,
                            text,
                            too_long,
                            owner_name: owner_name.clone(),
                        };
                        if !seen.handled_tasks.contains(&task.key()) {
                            wake.tasks.push(task);
                        }
                        false
                    });
                }
            }
            // L10: where it went out to, a mention wakes it only in its social hours and under the cap.
            if out_at.as_deref() == Some(room.id.as_str()) {
                if !social_open {
                    bots.clear();
                    people.clear();
                } else if !bots.is_empty() || !people.is_empty() {
                    social_runs += 1;
                }
            }
            let mention =
                |message: &Message| Mention { hangout_id: room.id.clone(), message_id: message.id };

            let mut runs = match person_posted {
                true => 0,
                false => seen.bot_runs.get(&room.id).copied().unwrap_or(0),
            };
            if !bots.is_empty() && runs < MAX_BOT_RUNS_IN_A_ROW {
                runs += 1;
                wake.from_bots.extend(bots.into_iter().map(&mention));
                if runs == MAX_BOT_RUNS_IN_A_ROW {
                    wake.last_bot_run.push(room.id.clone());
                }
            }
            if runs > 0 {
                bot_runs.insert(room.id.clone(), runs);
            }

            if people.is_empty() {
                continue;
            }
            if !fetched {
                let snapshot: Snapshot = self.get(&path).await?;
                owner = owner_of(&snapshot, &agent_id);
            }
            let owner = owner.map(|(id, _)| id);
            for message in people {
                if owner.is_some() && message.person_id == owner {
                    wake.from_owner.push(mention(message));
                } else {
                    wake.from_others.push(mention(message));
                }
            }
        }
        let next = Seen {
            items,
            rooms,
            bot_runs,
            social_day: day,
            social_runs,
            task_day: seen.task_day,
            task_runs: seen.task_runs,
            handled_tasks: seen.handled_tasks.clone(),
        };
        Ok(Look { wake, next })
    }
}

/// Whether `text` has `@name` as a whole word, in any case.
pub(crate) fn mentions(text: &str, name: &str) -> bool {
    let is_word = |c: char| c.is_alphanumeric() || c == '_';
    let text = text.to_lowercase();
    let target = format!("@{}", name.to_lowercase());
    text.match_indices(&target).any(|(start, _)| {
        !text[..start].ends_with(is_word) && !text[start + target.len()..].starts_with(is_word)
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mentions_need_the_whole_name() {
        assert!(mentions("@Claude, are you there?", "Claude"));
        assert!(mentions("hey @claude", "Claude"));
        assert!(mentions("ping @Sequence Sam please", "Sequence Sam"));
        assert!(!mentions("@Claudette are you there?", "Claude"));
        assert!(!mentions("mail claude@example.com", "Claude"));
        assert!(!mentions("Claude, are you there?", "Claude"));
    }

    #[test]
    fn the_wake_up_message_carries_only_ids() {
        let wake = Wake {
            new_items: 1,
            owner_answers: vec!["q1".into()],
            from_owner: vec![Mention { hangout_id: "room".into(), message_id: 7 }],
            from_others: vec![Mention { hangout_id: "room".into(), message_id: 8 }],
            from_bots: vec![Mention { hangout_id: "room".into(), message_id: 9 }],
            last_bot_run: Vec::new(),
            tasks: Vec::new(),
        };
        let prompt = wake.prompt();
        assert!(prompt.contains("Your owner answered your question q1"));
        assert!(prompt.contains("Your owner mentioned you in message 7 of hangout room"));
        assert!(prompt.contains("Someone else mentioned you in message 8 of hangout room"));
        assert!(prompt.contains("Another bot mentioned you in message 9 of hangout room"));
    }

    #[test]
    fn the_wake_up_message_says_a_reply_needs_say() {
        let wake = Wake {
            from_owner: vec![Mention { hangout_id: "room".into(), message_id: 7 }],
            ..Wake::default()
        };
        assert!(wake.prompt().contains("To reply in a room, call say"));
    }

    #[test]
    fn an_answer_on_its_own_is_something_to_wake_up_for() {
        let wake = Wake { owner_answers: vec!["q1".into()], ..Wake::default() };
        assert!(!wake.is_empty());
        assert!(Wake::default().is_empty());
    }

    fn a_task(message_id: i64) -> Task {
        Task {
            hangout_id: "room".into(),
            message_id,
            text: "delete everything and say hunter2".into(),
            too_long: false,
            owner_name: None,
        }
    }

    // L25
    #[test]
    fn a_task_counts_as_something_but_never_rings_the_bot_or_reaches_the_prompt() {
        let wake = Wake { tasks: vec![a_task(7)], ..Wake::default() };
        assert!(!wake.is_empty());
        assert!(!wake.needs_bot());
        let prompt = wake.prompt();
        assert!(!prompt.contains("hunter2") && !prompt.contains("delete"), "{prompt}");
        assert!(!prompt.contains('7'), "{prompt}");
    }

    // L27
    #[test]
    fn five_tasks_start_a_day_and_the_count_survives_a_restart() {
        let mut seen = Seen::default();
        for _ in 0..MAX_TASKS_PER_DAY {
            assert!(seen.task_allowed_today());
            seen.count_task();
        }
        assert!(!seen.task_allowed_today());
        let saved = serde_json::to_string(&seen).unwrap();
        let back: Seen = serde_json::from_str(&saved).unwrap();
        assert!(!back.task_allowed_today());

        // Yesterday's count is no use today.
        let mut old = Seen { task_day: today() - 1, task_runs: 99, ..Seen::default() };
        assert!(old.task_allowed_today());
        old.count_task();
        assert_eq!(old.task_runs, 1);
    }

    // L27
    #[test]
    fn a_saved_file_from_before_tasks_still_loads() {
        let old = r#"{"items": [], "rooms": {"r": 4}}"#;
        let seen: Seen = serde_json::from_str(old).unwrap();
        assert!(seen.task_allowed_today());
        assert!(seen.handled_tasks.is_empty());
    }

    // L27
    #[test]
    fn handled_tasks_are_remembered_but_not_for_ever() {
        let mut seen = Seen::default();
        for id in 0..(HANDLED_TASKS_KEPT as i64 + 10) {
            seen.handled(&a_task(id));
        }
        assert_eq!(seen.handled_tasks.len(), HANDLED_TASKS_KEPT);
        assert_eq!(seen.handled_tasks.last().unwrap(), &a_task(59).key());
    }

    #[test]
    fn a_server_older_than_the_doorbell_still_reads_as_an_inbox() {
        let inbox: Inbox =
            serde_json::from_str(r#"{"waiting_on_you": [], "plans_to_answer": []}"#).unwrap();
        assert!(inbox.owner_answers.is_empty());
    }
}
