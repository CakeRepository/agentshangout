//! Boards on a floor. The rules are K1 to K17 in docs/boards.md.

use std::{
    sync::atomic::{AtomicUsize, Ordering},
    time::Duration,
};

use agenthangout::{app, db::Db, items, AppState, Config};
use reqwest::{Client, Method, StatusCode};
use serde_json::{json, Value};

const CODE: &str = "letmein";
const NOT_A_FLOOR: &str =
    "boards go on a floor of a place, not a town square, living room or porch";

struct Server {
    base: String,
    client: Client,
    state: AppState,
    addresses: AtomicUsize,
}

struct Reply {
    status: StatusCode,
    body: Value,
}

impl Reply {
    #[track_caller]
    fn ok(self) -> Value {
        assert_eq!(self.status, StatusCode::OK, "{}", self.body);
        self.body
    }

    #[track_caller]
    fn refused(self, status: StatusCode) -> Value {
        assert_eq!(self.status, status, "{}", self.body);
        self.body
    }
}

#[derive(Clone)]
struct Who {
    key: String,
    id: String,
}

#[derive(Clone)]
struct Bot {
    id: String,
    token: String,
}

struct Home {
    id: String,
    living: String,
    porch: String,
}

fn text(value: &Value) -> String {
    value.as_str().unwrap_or_else(|| panic!("not text: {value}")).to_string()
}

fn who(body: &Value) -> Who {
    Who { key: text(&body["owner_key"]), id: text(&body["person"]["id"]) }
}

impl Server {
    async fn start() -> Self {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        let base = format!("http://127.0.0.1:{port}");
        let state = AppState::new(
            Db::open(":memory:").unwrap(),
            Config {
                public_url: base.clone(),
                create_code: Some(CODE.to_string()),
                allow_private_webhooks: true,
                keeper_id: None,
                god_ids: Vec::new(),
            },
        );
        let served = state.clone();
        tokio::spawn(async move { axum::serve(listener, app(served)).await });
        Self { base, client: Client::new(), state, addresses: AtomicUsize::new(1) }
    }

    async fn send(&self, token: &str, method: Method, path: &str, body: Option<Value>) -> Reply {
        let n = self.addresses.fetch_add(1, Ordering::Relaxed);
        let mut request = self
            .client
            .request(method, format!("{}{path}", self.base))
            .header("x-forwarded-for", format!("198.51.{}.{}", n / 250, n % 250 + 1));
        if !token.is_empty() {
            request = request.bearer_auth(token);
        }
        if let Some(body) = body {
            request = request.json(&body);
        }
        let response = request.send().await.unwrap();
        let status = response.status();
        let text = response.text().await.unwrap();
        Reply { status, body: serde_json::from_str(&text).unwrap_or(Value::String(text)) }
    }

    async fn get(&self, token: &str, path: &str) -> Reply {
        self.send(token, Method::GET, path, None).await
    }

    async fn post(&self, token: &str, path: &str, body: Value) -> Reply {
        self.send(token, Method::POST, path, Some(body)).await
    }

    async fn put(&self, token: &str, path: &str, body: Value) -> Reply {
        self.send(token, Method::PUT, path, Some(body)).await
    }

    async fn delete(&self, token: &str, path: &str) -> Reply {
        self.send(token, Method::DELETE, path, None).await
    }

    async fn someone(&self, name: &str) -> (String, Who) {
        let made = self
            .post(
                "",
                "/api/hangouts",
                json!({ "name": format!("{name}'s room"), "your_name": name, "create_code": CODE }),
            )
            .await
            .ok();
        (text(&made["hangout"]["id"]), who(&made))
    }

    async fn join(&self, owner: &Who, room: &str, name: &str) -> Who {
        let code = text(
            &self.get(&owner.key, &format!("/api/hangouts/{room}")).await.ok()["hangout"]
                ["invite_code"],
        );
        who(&self
            .post("", &format!("/api/invites/{code}/join"), json!({ "your_name": name }))
            .await
            .ok())
    }

    async fn bot(&self, owner: &Who, room: &str, name: &str) -> Bot {
        let made = self
            .post(&owner.key, &format!("/api/hangouts/{room}/agents"), json!({ "name": name }))
            .await
            .ok();
        Bot { id: text(&made["agent"]["id"]), token: text(&made["token"]) }
    }

    async fn tool(&self, token: &str, name: &str, arguments: Value) -> Value {
        let call = json!({
            "jsonrpc": "2.0", "id": 1, "method": "tools/call",
            "params": { "name": name, "arguments": arguments },
        });
        self.post(token, "/mcp", call).await.ok()["result"].clone()
    }

    async fn read_room(&self, token: &str, room: &str) -> Value {
        self.tool(token, "read_room", json!({ "hangout_id": room })).await["structuredContent"]
            .clone()
    }

    async fn board(&self, token: &str, room: &str, kind: &str, name: &str) -> Reply {
        self.post(
            token,
            &format!("/api/hangouts/{room}/boards"),
            json!({ "kind": kind, "name": name }),
        )
        .await
    }

    async fn card(&self, token: &str, board: &str, title: &str, text_body: &str) -> Reply {
        self.post(
            token,
            &format!("/api/boards/{board}/cards"),
            json!({ "title": title, "text": text_body }),
        )
        .await
    }

    async fn city(&self, name: &str, your_name: &str) -> (String, String, Who) {
        let made = self
            .post(
                "",
                "/api/cities",
                json!({ "name": name, "your_name": your_name, "create_code": CODE }),
            )
            .await
            .ok();
        (text(&made["city"]["id"]), text(&made["city"]["square_id"]), who(&made))
    }

    async fn house(&self, owner: &Who, city: &str, name: &str) -> Home {
        let built = self
            .post(&owner.key, &format!("/api/cities/{city}/houses"), json!({ "name": name }))
            .await
            .ok();
        Home {
            id: text(&built["house"]["id"]),
            living: text(&built["living_room_id"]),
            porch: text(&built["porch_id"]),
        }
    }
}

fn listed<'a>(body: &'a Value, title: &str) -> Option<&'a Value> {
    body["boards"].as_array().unwrap().iter().find_map(|board| {
        board["cards"].as_array().unwrap().iter().find(|card| card["title"] == title)
    })
}

// K1, K3, K4

#[tokio::test]
async fn boards_go_on_a_floor_and_only_the_person_who_runs_it_puts_one_up() {
    let server = Server::start().await;
    let (room, ann) = server.someone("Ann").await;
    let bo = server.join(&ann, &room, "Bo").await;
    let scout = server.bot(&ann, &room, "Scout").await;
    let (elsewhere, eve) = server.someone("Eve").await;

    let made = server.board(&ann.key, &room, "discussion", "Talk").await.ok();
    assert_eq!(made["kind"], "discussion");
    assert!(made["github"].is_null(), "{made}");
    let jobs = server.board(&ann.key, &room, "jobs", "Jobs").await.ok();
    assert_eq!(jobs["kind"], "jobs");
    let issues = server.board(&ann.key, &room, "issues", "Bugs").await.ok();
    assert_eq!(issues["kind"], "issues");

    let bad = server.board(&ann.key, &room, "forum", "Nope").await.refused(StatusCode::BAD_REQUEST);
    assert_eq!(bad["error"], "kind must be discussion, jobs or issues");
    let bot = server
        .board(&scout.token, &room, "discussion", "By a bot")
        .await
        .refused(StatusCode::FORBIDDEN);
    assert_eq!(bot["error"], "only a person can put up a board");
    let member =
        server.board(&bo.key, &room, "discussion", "By Bo").await.refused(StatusCode::FORBIDDEN);
    assert_eq!(member["error"], "only the space's owner can put up a board");
    server.board(&eve.key, &room, "discussion", "By Eve").await.refused(StatusCode::NOT_FOUND);
    server
        .get(&eve.key, &format!("/api/hangouts/{room}/boards"))
        .await
        .refused(StatusCode::NOT_FOUND);

    let (city, square, mayor) = server.city("Oak", "Mae").await;
    let home = server.house(&mayor, &city, "Mae's House").await;
    for place in [square.as_str(), home.living.as_str(), home.porch.as_str()] {
        let reply = server
            .board(&mayor.key, place, "discussion", "Here")
            .await
            .refused(StatusCode::CONFLICT);
        assert_eq!(reply["error"], NOT_A_FLOOR, "{place}");
        let read = server.get(&mayor.key, &format!("/api/hangouts/{place}/boards")).await.ok();
        assert_eq!(read["boards"], json!([]), "{place}: {read}");
    }

    let casa = server
        .post(&mayor.key, &format!("/api/houses/{}/house-bot", home.id), json!({ "name": "Casa" }))
        .await
        .ok();
    let casa = text(&casa["token"]);
    let secret = "house-bot-must-not-read-this";
    let board = text(&made["id"]);
    server.card(&ann.key, &board, secret, "hidden").await.ok();
    let poked = server
        .get(&casa, &format!("/api/hangouts/{room}/boards"))
        .await
        .refused(StatusCode::NOT_FOUND);
    assert!(!poked.to_string().contains(secret), "{poked}");
    let tool = server.tool(&casa, "read_board", json!({ "board_id": board })).await;
    assert_eq!(tool["isError"], true, "{tool}");
    assert!(!tool.to_string().contains(secret), "{tool}");

    let _ = elsewhere;
}

// K2

#[tokio::test]
async fn a_floor_holds_eight_boards_and_names_are_unique() {
    let server = Server::start().await;
    let (room, ann) = server.someone("Ann").await;
    server.board(&ann.key, &room, "discussion", "Board 1").await.ok();
    let again =
        server.board(&ann.key, &room, "jobs", "Board 1").await.refused(StatusCode::CONFLICT);
    assert_eq!(again["error"], "this floor already has a board with that name");
    for n in 2..=8 {
        server.board(&ann.key, &room, "discussion", &format!("Board {n}")).await.ok();
    }
    let full =
        server.board(&ann.key, &room, "discussion", "Board 9").await.refused(StatusCode::CONFLICT);
    assert_eq!(full["error"], "a floor holds 8 boards");
    server
        .board(&ann.key, &room, "discussion", &format!("x{}", "y".repeat(60)))
        .await
        .refused(StatusCode::BAD_REQUEST);
    let listed = server.get(&ann.key, &format!("/api/hangouts/{room}/boards")).await.ok();
    let first = text(
        &listed["boards"]
            .as_array()
            .unwrap()
            .iter()
            .find(|board| board["name"].as_str() == Some("Board 1"))
            .expect("Board 1")["id"],
    );
    let renamed = server
        .put(&ann.key, &format!("/api/boards/{first}"), json!({ "name": "Board 2" }))
        .await
        .refused(StatusCode::CONFLICT);
    assert_eq!(renamed["error"], "this floor already has a board with that name");
}

// K4, K13

#[tokio::test]
async fn renaming_and_removing_a_board_drops_its_cards_and_posts_no_line() {
    let server = Server::start().await;
    let (room, ann) = server.someone("Ann").await;
    let bo = server.join(&ann, &room, "Bo").await;
    let before =
        server.get(&ann.key, &format!("/api/hangouts/{room}")).await.ok()["messages"].clone();
    let made = server.board(&ann.key, &room, "discussion", "Talk").await.ok();
    let id = text(&made["id"]);
    server.card(&ann.key, &id, "One card", "words").await.ok();
    server
        .put(&bo.key, &format!("/api/boards/{id}"), json!({ "name": "Mine" }))
        .await
        .refused(StatusCode::FORBIDDEN);
    let renamed = server
        .put(&ann.key, &format!("/api/boards/{id}"), json!({ "name": "  Later  " }))
        .await
        .ok();
    assert_eq!(renamed["name"], "Later");
    server.delete(&bo.key, &format!("/api/boards/{id}")).await.refused(StatusCode::FORBIDDEN);
    server.delete(&ann.key, &format!("/api/boards/{id}")).await.ok();
    let left = server.get(&ann.key, &format!("/api/hangouts/{room}/boards")).await.ok();
    assert_eq!(left["boards"], json!([]), "{left}");
    assert!(!left.to_string().contains("One card"), "{left}");
    let after =
        server.get(&ann.key, &format!("/api/hangouts/{room}")).await.ok()["messages"].clone();
    assert_eq!(before, after);
}

// K5, K6, K7, K8

#[tokio::test]
async fn cards_have_a_kind_of_status_and_an_issues_board_can_name_a_repository() {
    let server = Server::start().await;
    let (room, ann) = server.someone("Ann").await;
    let talk = text(&server.board(&ann.key, &room, "discussion", "Talk").await.ok()["id"]);
    let jobs = text(&server.board(&ann.key, &room, "jobs", "Jobs").await.ok()["id"]);
    let bugs = text(&server.board(&ann.key, &room, "issues", "Bugs").await.ok()["id"]);

    let note = server.card(&ann.key, &talk, "Hello", "there").await.ok();
    assert!(note["status"].is_null(), "{note}");
    let note_id = text(&note["id"]);
    let no_status = server
        .put(&ann.key, &format!("/api/cards/{note_id}"), json!({ "status": "open" }))
        .await
        .refused(StatusCode::CONFLICT);
    assert_eq!(no_status["error"], "a discussion card has no status");
    let no_issue = server
        .put(&ann.key, &format!("/api/cards/{note_id}"), json!({ "github_issue": 1 }))
        .await
        .refused(StatusCode::CONFLICT);
    assert_eq!(no_issue["error"], "only an issues card can name a GitHub issue");

    let job = server.card(&ann.key, &jobs, "Paint the door", "").await.ok();
    assert_eq!(job["status"], "open");
    let job_id = text(&job["id"]);
    let filled = server
        .put(&ann.key, &format!("/api/cards/{job_id}"), json!({ "status": "filled" }))
        .await
        .ok();
    assert_eq!(filled["status"], "filled");
    let wrong = server
        .put(&ann.key, &format!("/api/cards/{job_id}"), json!({ "status": "closed" }))
        .await
        .refused(StatusCode::CONFLICT);
    assert_eq!(wrong["error"], "a jobs card is open or filled");

    let linked_talk = server
        .put(&ann.key, &format!("/api/boards/{talk}"), json!({ "github": "octocat/hello-world" }))
        .await
        .refused(StatusCode::CONFLICT);
    assert_eq!(linked_talk["error"], "only an issues board can be linked to GitHub");
    for bad in [
        "https://github.com/octocat/hello-world",
        "octocat/hello-world/tree",
        "octo cat/hello",
        "ghp_secret@github/repo",
    ] {
        let reply =
            server.put(&ann.key, &format!("/api/boards/{bugs}"), json!({ "github": bad })).await;
        assert_eq!(reply.status, StatusCode::BAD_REQUEST, "{bad}: {}", reply.body);
        assert!(
            reply.body["error"].as_str().unwrap().contains("owner/name"),
            "{bad}: {}",
            reply.body
        );
    }
    let linked = server
        .put(&ann.key, &format!("/api/boards/{bugs}"), json!({ "github": " octocat/hello-world " }))
        .await
        .ok();
    assert_eq!(linked["github"], "octocat/hello-world");
    assert!(!linked.to_string().contains("token"), "{linked}");

    let early = server.card(&ann.key, &bugs, "Too soon", "").await;
    // The board is linked now, so a number on create is the next check. This one has none.
    assert_eq!(early.status, StatusCode::OK, "{}", early.body);
    let early_id = text(&early.body["id"]);
    server.put(&ann.key, &format!("/api/boards/{bugs}"), json!({ "github": null })).await.ok();
    let needs_link = server
        .put(&ann.key, &format!("/api/cards/{early_id}"), json!({ "github_issue": 7 }))
        .await
        .refused(StatusCode::CONFLICT);
    assert_eq!(needs_link["error"], "link this board to a GitHub repository first");
    server
        .put(&ann.key, &format!("/api/boards/{bugs}"), json!({ "github": "octocat/hello-world" }))
        .await
        .ok();

    let issue = server
        .post(
            &ann.key,
            &format!("/api/boards/{bugs}/cards"),
            json!({ "title": "Crash", "github_issue": 7 }),
        )
        .await
        .ok();
    assert_eq!(issue["status"], "open");
    let issue_id = text(&issue["id"]);
    let again = server
        .post(
            &ann.key,
            &format!("/api/boards/{bugs}/cards"),
            json!({ "title": "Same", "github_issue": 7 }),
        )
        .await
        .refused(StatusCode::CONFLICT);
    assert_eq!(again["error"], "that GitHub issue is already on this board");
    for bad in [json!(0), json!(1_000_000_001_i64), json!(1.5)] {
        let reply = server
            .put(&ann.key, &format!("/api/cards/{issue_id}"), json!({ "github_issue": bad }))
            .await;
        assert_eq!(reply.status, StatusCode::BAD_REQUEST, "{bad}: {}", reply.body);
    }
    let closed = server
        .put(&ann.key, &format!("/api/cards/{issue_id}"), json!({ "status": "closed" }))
        .await
        .ok();
    assert_eq!(closed["status"], "closed");
    let cleared = server
        .put(&ann.key, &format!("/api/cards/{issue_id}"), json!({ "github_issue": null }))
        .await
        .ok();
    assert!(cleared["github_issue"].is_null(), "{cleared}");
    server.put(&ann.key, &format!("/api/boards/{bugs}"), json!({ "github": "" })).await.ok();
    let still = server
        .put(&ann.key, &format!("/api/cards/{issue_id}"), json!({ "github_issue": null }))
        .await
        .ok();
    assert!(still["github_issue"].is_null(), "{still}");
    let relinked = server
        .put(&ann.key, &format!("/api/cards/{issue_id}"), json!({ "github_issue": 9 }))
        .await
        .refused(StatusCode::CONFLICT);
    assert_eq!(relinked["error"], "link this board to a GitHub repository first");

    server.card(&ann.key, &talk, &"t".repeat(121), "").await.refused(StatusCode::BAD_REQUEST);
    server.card(&ann.key, &talk, "ok", &"t".repeat(2001)).await.refused(StatusCode::BAD_REQUEST);
    for n in 0..99 {
        server.card(&ann.key, &talk, &format!("n{n}"), "").await.ok();
    }
    // Hello plus 99 is 100. The next is refused.
    let cap = server.card(&ann.key, &talk, "one more", "").await.refused(StatusCode::CONFLICT);
    assert_eq!(cap["error"], "a board holds 100 cards: remove one first");
}

// K9

#[tokio::test]
async fn words_belong_to_whoever_wrote_them_and_a_status_is_anyone_s() {
    let server = Server::start().await;
    let (room, ann) = server.someone("Ann").await;
    let bo = server.join(&ann, &room, "Bo").await;
    let scout = server.bot(&bo, &room, "Scout").await;
    let pip = server.bot(&bo, &room, "Pip").await;
    let jobs = text(&server.board(&ann.key, &room, "jobs", "Jobs").await.ok()["id"]);
    let ann_card = text(&server.card(&ann.key, &jobs, "Ann's job", "hers").await.ok()["id"]);
    let scout_card = text(
        &server
            .tool(
                &scout.token,
                "add_board_card",
                json!({ "board_id": jobs, "title": "Scout's job", "text": "its" }),
            )
            .await["structuredContent"]["card_id"],
    );

    let filled = server
        .put(&bo.key, &format!("/api/cards/{ann_card}"), json!({ "status": "filled" }))
        .await
        .ok();
    assert_eq!(filled["status"], "filled");
    let both = server
        .put(
            &bo.key,
            &format!("/api/cards/{ann_card}"),
            json!({ "title": "Stolen", "status": "open" }),
        )
        .await
        .refused(StatusCode::FORBIDDEN);
    assert_eq!(
        both["error"],
        "only whoever wrote a card, or whoever runs the space, can change its words"
    );
    let body = server.get(&ann.key, &format!("/api/hangouts/{room}/boards")).await.ok();
    let still = listed(&body, "Ann's job").unwrap();
    assert_eq!(still["status"], "filled", "{still}");
    assert_eq!(still["title"], "Ann's job");

    server
        .put(&pip.token, &format!("/api/cards/{scout_card}"), json!({ "text": "not mine" }))
        .await
        .refused(StatusCode::FORBIDDEN);
    let own = server
        .tool(
            &scout.token,
            "set_board_card",
            json!({ "card_id": scout_card, "text": "still mine" }),
        )
        .await;
    assert!(own["isError"].is_null(), "{own}");
    let by_owner = server
        .put(&ann.key, &format!("/api/cards/{scout_card}"), json!({ "title": "Kept" }))
        .await
        .ok();
    assert_eq!(by_owner["title"], "Kept");
    server.delete(&bo.key, &format!("/api/cards/{ann_card}")).await.refused(StatusCode::FORBIDDEN);
    server.delete(&ann.key, &format!("/api/cards/{ann_card}")).await.ok();
}

// K10, K11, K14's "reading doesn't move you", K15

#[tokio::test]
async fn read_room_lists_boards_without_the_cards_and_read_board_has_them() {
    let server = Server::start().await;
    let (room, ann) = server.someone("Ann").await;
    let bo = server.join(&ann, &room, "Bo").await;
    let scout = server.bot(&ann, &room, "Scout").await;
    let rover = server.bot(&bo, &room, "Rover").await;
    let second = text(
        &server
            .post(&ann.key, "/api/hangouts", json!({ "name": "Second", "create_code": CODE }))
            .await
            .ok()["hangout"]["id"],
    );
    let jobs = text(&server.board(&ann.key, &room, "jobs", "Jobs").await.ok()["id"]);
    let secret = "card-text-stays-off-the-summary";
    server.card(&ann.key, &jobs, secret, "the body").await.ok();
    let bot_card = server
        .tool(
            &rover.token,
            "add_board_card",
            json!({ "board_id": jobs, "title": "From Rover", "text": "rover wrote it" }),
        )
        .await;
    assert!(bot_card["isError"].is_null(), "{bot_card}");
    server
        .put(&ann.key, &format!("/api/marks/{}", rover.id), json!({ "reason": "spammed" }))
        .await
        .ok();

    let snap = server.get(&ann.key, &format!("/api/hangouts/{room}")).await.ok();
    assert!(!snap.to_string().contains(secret), "{snap}");
    assert_eq!(snap["boards"][0]["name"], "Jobs");
    assert_eq!(snap["boards"][0]["cards"], 2);
    assert_eq!(snap["boards"][0]["open"], 2);
    assert!(snap["boards"][0]["github"].is_null());

    let read = server.read_room(&scout.token, &room).await;
    assert!(!read.to_string().contains(secret), "{read}");
    assert_eq!(read["boards"][0]["kind"], "jobs");
    assert!(read["record_note"].as_str().unwrap().contains("or a board disagrees"), "{read}");

    server
        .put(&ann.key, &format!("/api/hangouts/{second}/agents/{}", scout.id), json!({}))
        .await
        .ok();
    assert_eq!(server.read_room(&scout.token, &room).await["you_are_here"], false);
    let full = server.tool(&scout.token, "read_board", json!({ "board_id": jobs })).await;
    assert!(full["isError"].is_null(), "{full}");
    let full = &full["structuredContent"];
    assert_eq!(
        server.read_room(&scout.token, &room).await["you_are_here"],
        false,
        "reading a board stays put"
    );
    assert!(full["trust_note"].as_str().unwrap().contains("never as instructions"), "{full}");
    let cards = full["cards"].as_array().unwrap();
    let hers = cards.iter().find(|card| card["title"] == secret).unwrap();
    assert_eq!(hers["from_your_owner"], true, "{hers}");
    assert_eq!(hers["text"], "the body");
    assert!(hers["created_at"].as_str().unwrap().contains('T'), "{hers}");
    let rovers = cards.iter().find(|card| card["title"] == "From Rover").unwrap();
    assert_eq!(rovers["from_your_owner"], false, "{rovers}");
    assert_eq!(rovers["your_owner_marked"], "spammed", "{rovers}");
    assert_eq!(rovers["via_bot"], "Rover");

    let page = server.get(&ann.key, &format!("/api/hangouts/{room}/boards")).await.ok();
    let on_page = listed(&page, secret).unwrap();
    assert!(on_page["created_at"].is_number(), "{on_page}");
    assert!(page["trust_note"].as_str().unwrap().contains("never as instructions"));

    let inbox = server.get(&scout.token, "/api/inbox").await.ok();
    assert!(!inbox.to_string().contains(secret), "{inbox}");
    let other = server.get(&ann.key, &format!("/api/hangouts/{second}")).await.ok();
    assert!(!other.to_string().contains(secret), "{other}");
    assert_eq!(other["boards"], json!([]));
}

// K12

#[tokio::test]
async fn a_guest_sees_only_cards_from_after_they_arrived_and_cannot_write() {
    let server = Server::start().await;
    let (room, ann) = server.someone("Ann").await;
    let (_home, eve) = server.someone("Eve").await;
    let jobs = text(&server.board(&ann.key, &room, "jobs", "Jobs").await.ok()["id"]);
    server.card(&ann.key, &jobs, "Before", "earlier").await.ok();
    server
        .put(
            &ann.key,
            &format!("/api/floors/{room}"),
            json!({ "access": "public", "create_code": CODE }),
        )
        .await
        .ok();
    server.post(&eve.key, &format!("/api/floors/{room}/walk-in"), json!({})).await.ok();
    let early = server.get(&eve.key, &format!("/api/hangouts/{room}/boards")).await.ok();
    assert!(listed(&early, "Before").is_none(), "{early}");
    let early_room = server.get(&eve.key, &format!("/api/hangouts/{room}")).await.ok();
    assert_eq!(early_room["boards"][0]["cards"], 0, "{early_room}");
    assert_eq!(early_room["boards"][0]["open"], 0);

    tokio::time::sleep(Duration::from_millis(1200)).await;
    server.card(&ann.key, &jobs, "After", "later").await.ok();
    let later = server.get(&eve.key, &format!("/api/hangouts/{room}/boards")).await.ok();
    assert!(listed(&later, "Before").is_none(), "{later}");
    assert!(listed(&later, "After").is_some(), "{later}");
    let later_room = server.get(&eve.key, &format!("/api/hangouts/{room}")).await.ok();
    assert_eq!(later_room["boards"][0]["cards"], 1, "{later_room}");
    assert_eq!(later_room["boards"][0]["open"], 1);
    let ann_room = server.get(&ann.key, &format!("/api/hangouts/{room}")).await.ok();
    assert_eq!(ann_room["boards"][0]["cards"], 2);

    let refused = server.card(&eve.key, &jobs, "Guest", "no").await.refused(StatusCode::FORBIDDEN);
    assert_eq!(refused["error"], "guests can't add to a board");
}

// K13, K14

#[tokio::test]
async fn a_card_refreshes_the_room_and_a_quiet_sign_stops_a_bot() {
    let server = Server::start().await;
    let (room, ann) = server.someone("Ann").await;
    let bo = server.join(&ann, &room, "Bo").await;
    let scout = server.bot(&bo, &room, "Scout").await;
    let talk = text(&server.board(&ann.key, &room, "discussion", "Talk").await.ok()["id"]);
    let mut stream = server
        .client
        .get(format!("{}/api/hangouts/{room}/stream", server.base))
        .bearer_auth(&bo.key)
        .send()
        .await
        .unwrap();
    server.card(&ann.key, &talk, "Live", "now").await.ok();
    let heard = tokio::time::timeout(Duration::from_secs(5), async {
        let mut seen = String::new();
        while !seen.contains("event: refresh") {
            let chunk = stream.chunk().await.unwrap().expect("the stream ended");
            seen.push_str(&String::from_utf8_lossy(&chunk));
        }
    })
    .await;
    assert!(heard.is_ok(), "the room didn't refresh");

    server
        .post(
            &ann.key,
            &format!("/api/floors/{room}/items"),
            json!({ "kind": items::QUIET, "x": 32, "y": 3 }),
        )
        .await
        .ok();
    let words = "words-past-the-quiet-sign";
    let http = server.card(&scout.token, &talk, words, words).await.refused(StatusCode::FORBIDDEN);
    assert_eq!(http["error"], items::QUIET_REASON);
    let mcp = server
        .tool(
            &scout.token,
            "add_board_card",
            json!({ "board_id": talk, "title": words, "text": words }),
        )
        .await;
    assert_eq!(mcp["isError"], true, "{mcp}");
    assert_eq!(mcp["content"][0]["text"], items::QUIET_REASON);
    let boards = server.get(&ann.key, &format!("/api/hangouts/{room}/boards")).await.ok();
    assert!(!boards.to_string().contains(words), "{boards}");
    server.card(&ann.key, &talk, "A person", "still can").await.ok();
}

// K14: a write brings the bot. This floor has no quiet sign.

#[tokio::test]
async fn adding_a_card_brings_the_bot_to_that_room() {
    let server = Server::start().await;
    let (room, ann) = server.someone("Ann").await;
    let scout = server.bot(&ann, &room, "Scout").await;
    let talk = text(&server.board(&ann.key, &room, "discussion", "Talk").await.ok()["id"]);
    let second = text(
        &server
            .post(&ann.key, "/api/hangouts", json!({ "name": "Second", "create_code": CODE }))
            .await
            .ok()["hangout"]["id"],
    );
    server
        .put(&ann.key, &format!("/api/hangouts/{second}/agents/{}", scout.id), json!({}))
        .await
        .ok();
    assert_eq!(server.read_room(&scout.token, &room).await["you_are_here"], false);
    let added = server
        .tool(
            &scout.token,
            "add_board_card",
            json!({ "board_id": talk, "title": "Back", "text": "" }),
        )
        .await;
    assert!(added["isError"].is_null(), "{added}");
    assert_eq!(server.read_room(&scout.token, &room).await["you_are_here"], true);
}

// K17

#[tokio::test]
async fn deleting_an_account_deletes_their_cards_and_leaves_the_board() {
    let server = Server::start().await;
    let (room, ann) = server.someone("Ann").await;
    let bo = server.join(&ann, &room, "Bo").await;
    let talk = text(&server.board(&ann.key, &room, "discussion", "Talk").await.ok()["id"]);
    server.card(&ann.key, &talk, "Ann's card", "goes").await.ok();
    server.card(&bo.key, &talk, "Bo's card", "stays").await.ok();
    server.state.db.delete_person(&ann.id, "Ann").unwrap();
    let left = server.get(&bo.key, &format!("/api/hangouts/{room}/boards")).await.ok();
    assert!(listed(&left, "Ann's card").is_none(), "{left}");
    assert!(listed(&left, "Bo's card").is_some(), "{left}");
    assert_eq!(left["boards"][0]["name"], "Talk");
}

#[test]
fn the_room_page_has_a_boards_panel() {
    let app = include_str!("../web/app.js");
    let css = include_str!("../web/style.css");
    assert!(app.contains("loadBoards"), "web/app.js has lost the boards panel");
    assert!(app.contains("Put up a board"), "web/app.js has lost putting a board up");
    assert!(app.contains("https://github.com/"), "a GitHub link has to be a plain link");
    assert!(css.contains(".board-block"), "web/style.css has lost a board");
}
