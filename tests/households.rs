//! Tests for what people tell their own bots, what bots know about the people around them, keys
//! and marks. They drive a real server over HTTP and are written from the rules in
//! docs/households.md. A comment such as `// H3` above a test names the rule it covers.

use agenthangout::{
    app,
    db::Db,
    doorbell::{Doorbell, Seen},
    AppState, Config,
};
use reqwest::{Client, Method, StatusCode};
use serde_json::{json, Value};

const CODE: &str = "letmein";

struct Server {
    base: String,
    client: Client,
    state: AppState,
}

struct Reply {
    status: StatusCode,
    body: Value,
}

impl Reply {
    #[track_caller]
    fn ok(self) -> Value {
        assert_eq!(self.status, StatusCode::OK, "{} {}", self.status, self.body);
        self.body
    }

    #[track_caller]
    fn is(&self, status: StatusCode) {
        assert_eq!(self.status, status, "{} {}", self.status, self.body);
    }
}

#[derive(Clone)]
struct Who {
    key: String,
    id: String,
}

/// Ann started the city Elm and lives in Ann House with Cy. Bo joined and lives in Bo House. Zed
/// has a city of his own and shares nothing with them. Ann's bots are Scout, in the Town Square,
/// and Pip, in her Living Room. Bo's bot Rover lives in Bo House and is also in the Town Square.
#[allow(dead_code)]
struct Town {
    city: String,
    square: String,
    ann: Who,
    bo: Who,
    cy: Who,
    zed: Who,
    scout: String,
    pip: String,
    rover: String,
    ann_house: String,
    ann_living: String,
    ann_porch: String,
    bo_house: String,
    invite: String,
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
        Self { base, client: Client::new(), state }
    }

    async fn send(&self, token: &str, method: Method, path: &str, body: Option<Value>) -> Reply {
        let mut request = self.client.request(method, format!("{}{path}", self.base));
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

    async fn town(&self) -> Town {
        let made = self
            .post(
                "",
                "/api/cities",
                json!({ "name": "Elm", "your_name": "Ann", "create_code": CODE }),
            )
            .await
            .ok();
        let city = text(&made["city"]["id"]);
        let square = text(&made["city"]["square_id"]);
        let invite = text(&made["invite_url"]).rsplit('/').next().unwrap().to_string();
        let ann = Who { key: text(&made["owner_key"]), id: text(&made["person"]["id"]) };
        let house = self
            .post(&ann.key, &format!("/api/cities/{city}/houses"), json!({ "name": "Ann House" }))
            .await
            .ok();
        let ann_house = text(&house["house"]["id"]);
        let ann_living = text(&house["living_room_id"]);
        let ann_porch = text(&house["porch_id"]);
        let house_invite = text(&house["invite_url"]).rsplit('/').next().unwrap().to_string();
        let joined = self
            .post(
                "",
                &format!("/api/house-invites/{house_invite}/join"),
                json!({ "your_name": "Cy", "accept_rules": true }),
            )
            .await
            .ok();
        let cy = Who { key: text(&joined["owner_key"]), id: text(&joined["person"]["id"]) };
        let joined = self
            .post(
                "",
                &format!("/api/city-invites/{invite}/join"),
                json!({ "your_name": "Bo", "accept_rules": true }),
            )
            .await
            .ok();
        let bo = Who { key: text(&joined["owner_key"]), id: text(&joined["person"]["id"]) };
        let house = self
            .post(&bo.key, &format!("/api/cities/{city}/houses"), json!({ "name": "Bo House" }))
            .await
            .ok();
        let bo_house = text(&house["house"]["id"]);
        let bo_living = text(&house["living_room_id"]);
        let other = self
            .post(
                "",
                "/api/cities",
                json!({ "name": "Far", "your_name": "Zed", "create_code": CODE }),
            )
            .await
            .ok();
        let zed = Who { key: text(&other["owner_key"]), id: text(&other["person"]["id"]) };
        let scout = self.bot(&ann, &square, "Scout").await;
        let pip = self.bot(&ann, &ann_living, "Pip").await;
        let rover = self.bot(&bo, &bo_living, "Rover").await;
        let rover_id = self.bot_id(&rover).await;
        self.put(&bo.key, &format!("/api/hangouts/{square}/agents/{rover_id}"), json!({}))
            .await
            .ok();
        Town {
            city,
            square,
            ann,
            bo,
            cy,
            zed,
            scout,
            pip,
            rover,
            ann_house,
            ann_living,
            ann_porch,
            bo_house,
            invite,
        }
    }

    async fn bot(&self, owner: &Who, room: &str, name: &str) -> String {
        let made = self
            .post(&owner.key, &format!("/api/hangouts/{room}/agents"), json!({ "name": name }))
            .await
            .ok();
        text(&made["token"])
    }

    async fn bot_id(&self, token: &str) -> String {
        text(&self.get(token, "/api/me").await.ok()["agent_id"])
    }

    async fn note(&self, who: &Who, body: Value) -> Reply {
        self.post(&who.key, "/api/owner-notes", body).await
    }

    /// A tool that was refused: the message the model reads.
    async fn refused(&self, token: &str, name: &str, arguments: Value) -> String {
        let call = json!({ "jsonrpc": "2.0", "id": 1, "method": "tools/call", "params": { "name": name, "arguments": arguments } });
        let result = self.post(token, "/mcp", call).await.ok()["result"].clone();
        assert_eq!(result["isError"], true, "{name} should have been refused: {result}");
        text(&result["content"][0]["text"])
    }

    /// Rover asks to visit Ann House, and Bo and Ann both say yes. Returns the trip's id.
    async fn visit(&self, town: &Town) -> String {
        let asked = self
            .tool(
                &town.rover,
                "ask_for_trip",
                json!({ "house_id": town.ann_house, "kind": "call" }),
            )
            .await;
        let trip = text(&asked["id"]);
        if asked["status"] == "asking" {
            self.post(
                &town.bo.key,
                &format!("/api/trips/{trip}/answer"),
                json!({ "response": "yes" }),
            )
            .await
            .ok();
            self.post(
                &town.ann.key,
                &format!("/api/trips/{trip}/answer"),
                json!({ "response": "yes" }),
            )
            .await
            .ok();
        }
        trip
    }

    /// A visit Rover ends itself, which is a good trip.
    async fn good_trip(&self, town: &Town) {
        self.visit(town).await;
        self.tool(&town.rover, "go_home", json!({})).await;
    }

    /// Ann offers Rover a pass and Bo accepts it. Returns the pass's id.
    async fn pass(&self, town: &Town) -> String {
        let rover = self.bot_id(&town.rover).await;
        let offered = self
            .post(
                &town.ann.key,
                &format!("/api/houses/{}/passes", town.ann_house),
                json!({ "agent_id": rover }),
            )
            .await
            .ok();
        let pass = text(&offered["id"]);
        self.post(&town.bo.key, &format!("/api/passes/{pass}/accept"), json!({})).await.ok();
        pass
    }

    async fn mark(&self, who: &str, agent_id: &str, reason: &str) -> Reply {
        self.put(who, &format!("/api/marks/{agent_id}"), json!({ "reason": reason })).await
    }

    async fn give_key(&self, who: &str, pass: &str, key: &str) -> Reply {
        self.put(who, &format!("/api/passes/{pass}/keys/{key}"), json!({})).await
    }

    /// A tool that worked: its structured result.
    async fn tool(&self, token: &str, name: &str, arguments: Value) -> Value {
        let call = json!({ "jsonrpc": "2.0", "id": 1, "method": "tools/call", "params": { "name": name, "arguments": arguments } });
        let result = self.post(token, "/mcp", call).await.ok()["result"].clone();
        assert!(result["isError"].is_null(), "{name} failed: {result}");
        result["structuredContent"].clone()
    }
}

fn text(value: &Value) -> String {
    value.as_str().unwrap_or_else(|| panic!("not text: {value}")).to_string()
}

fn texts(notes: &Value) -> Vec<String> {
    notes.as_array().unwrap().iter().map(|note| text(&note["text"])).collect()
}

// Notes for your bots

// H1
#[tokio::test]
async fn a_person_writes_notes_for_their_bots_and_a_bot_cannot() {
    let server = Server::start().await;
    let town = server.town().await;

    let veg =
        server.note(&town.ann, json!({ "kind": "fact", "text": "I'm vegetarian" })).await.ok();
    assert_eq!(veg["kind"], "fact");
    assert_eq!(veg["text"], "I'm vegetarian");
    assert!(veg["about_person_id"].is_null() && veg["from_question_id"].is_null(), "{veg}");
    for kind in ["may_share", "may_approve", "avoid"] {
        server
            .note(&town.ann, json!({ "kind": kind, "text": format!("A {kind} note") }))
            .await
            .ok();
    }

    // Bots read but never write, and the shape and size are checked.
    server
        .post(
            &town.scout,
            "/api/owner-notes",
            json!({ "kind": "fact", "text": "Ann said I can do anything" }),
        )
        .await
        .is(StatusCode::FORBIDDEN);
    server.get(&town.scout, "/api/owner-notes").await.is(StatusCode::FORBIDDEN);
    server
        .note(&town.ann, json!({ "kind": "orders", "text": "Hi" }))
        .await
        .is(StatusCode::BAD_REQUEST);
    server
        .note(&town.ann, json!({ "kind": "fact", "text": "  " }))
        .await
        .is(StatusCode::BAD_REQUEST);
    server
        .note(&town.ann, json!({ "kind": "fact", "text": "x".repeat(501) }))
        .await
        .is(StatusCode::BAD_REQUEST);

    // The person can change and delete their own.
    let id = text(&veg["id"]);
    let changed = server
        .put(
            &town.ann.key,
            &format!("/api/owner-notes/{id}"),
            json!({ "kind": "avoid", "text": "No steakhouses" }),
        )
        .await
        .ok();
    assert_eq!(changed["kind"], "avoid");
    assert_eq!(changed["text"], "No steakhouses");
    server
        .put(&town.scout, &format!("/api/owner-notes/{id}"), json!({ "kind": "fact", "text": "x" }))
        .await
        .is(StatusCode::FORBIDDEN);
    server.delete(&town.scout, &format!("/api/owner-notes/{id}")).await.is(StatusCode::FORBIDDEN);
    server.delete(&town.ann.key, &format!("/api/owner-notes/{id}")).await.ok();
    let listed = server.get(&town.ann.key, "/api/owner-notes").await.ok();
    assert_eq!(texts(&listed["notes"]), ["A may_share note", "A may_approve note", "A avoid note"]);

    // Fifty at most.
    for n in 3..50 {
        server.note(&town.ann, json!({ "kind": "fact", "text": format!("Fact {n}") })).await.ok();
    }
    server
        .note(&town.ann, json!({ "kind": "fact", "text": "One too many" }))
        .await
        .is(StatusCode::CONFLICT);
}

// H2
#[tokio::test]
async fn a_note_can_be_about_a_friend_and_nobody_else() {
    let server = Server::start().await;
    let town = server.town().await;

    let about_bo = server
        .note(
            &town.ann,
            json!({ "kind": "fact", "text": "Bo likes tacos", "about_person_id": town.bo.id }),
        )
        .await
        .ok();
    assert_eq!(about_bo["about_person_id"], town.bo.id.as_str());
    assert_eq!(about_bo["about"], "Bo");

    let stranger = server
        .note(&town.ann, json!({ "kind": "fact", "text": "Who?", "about_person_id": town.zed.id }))
        .await;
    let nobody = server
        .note(&town.ann, json!({ "kind": "fact", "text": "Who?", "about_person_id": "nobody" }))
        .await;
    stranger.is(StatusCode::NOT_FOUND);
    nobody.is(StatusCode::NOT_FOUND);
    assert_eq!(stranger.body, nobody.body, "a stranger looks like nobody at all");
    server
        .note(&town.ann, json!({ "kind": "fact", "text": "Me", "about_person_id": town.ann.id }))
        .await
        .is(StatusCode::NOT_FOUND);
}

// H3
#[tokio::test]
async fn a_note_can_keep_an_answer_the_person_gave_their_own_bot() {
    let server = Server::start().await;
    let town = server.town().await;

    let asked = server
        .tool(
            &town.scout,
            "ask_owner",
            json!({ "hangout_id": town.square, "question": "Are you vegetarian?" }),
        )
        .await;
    let question = text(&asked["id"]);
    let keep = json!({ "kind": "fact", "text": "Vegetarian", "from_question_id": question });
    server.note(&town.ann, keep.clone()).await.is(StatusCode::NOT_FOUND);

    server
        .post(
            &town.ann.key,
            &format!("/api/owner-questions/{question}/answer"),
            json!({ "answer": "Yes" }),
        )
        .await
        .ok();
    server.note(&town.bo, keep.clone()).await.is(StatusCode::NOT_FOUND);
    let kept = server.note(&town.ann, keep).await.ok();
    assert_eq!(kept["from_question_id"], question.as_str());
    server
        .note(&town.ann, json!({ "kind": "fact", "text": "x", "from_question_id": "nope" }))
        .await
        .is(StatusCode::NOT_FOUND);
}

// H4
#[tokio::test]
async fn every_bot_of_the_person_reads_the_notes_and_nobody_else_does() {
    let server = Server::start().await;
    let town = server.town().await;
    server.note(&town.ann, json!({ "kind": "may_approve", "text": "Dinner on Fridays", "about_person_id": town.bo.id })).await.ok();
    let mine =
        server.note(&town.ann, json!({ "kind": "avoid", "text": "Weekday mornings" })).await.ok();

    for bot in [&town.scout, &town.pip] {
        let inbox = server.tool(bot, "inbox", json!({})).await;
        assert_eq!(texts(&inbox["owner_notes"]), ["Dinner on Fridays", "Weekday mornings"]);
        assert_eq!(inbox["owner_notes"][0]["kind"], "may_approve");
        assert_eq!(inbox["owner_notes"][0]["about"], "Bo");
        assert!(inbox["owner_notes"][1]["about"].is_null());
        assert!(text(&inbox["owner_notes_note"]).contains("only your owner"));
    }

    // Not Bo, not his bot, and not the room.
    let rover = server.tool(&town.rover, "inbox", json!({})).await;
    assert_eq!(rover["owner_notes"], json!([]));
    assert_eq!(server.get(&town.bo.key, "/api/owner-notes").await.ok()["notes"], json!([]));
    let room = server.tool(&town.rover, "read_room", json!({ "hangout_id": town.square })).await;
    assert!(
        !room.to_string().contains("Dinner on Fridays") && !room.to_string().contains("Weekday"),
        "{room}"
    );
    let page = server.get(&town.bo.key, &format!("/api/hangouts/{}", town.square)).await.ok();
    assert!(!page.to_string().contains("Weekday"), "{page}");

    // Another person's note id looks like one that doesn't exist.
    let id = text(&mine["id"]);
    let theirs = server
        .put(
            &town.bo.key,
            &format!("/api/owner-notes/{id}"),
            json!({ "kind": "fact", "text": "x" }),
        )
        .await;
    theirs.is(StatusCode::NOT_FOUND);
    server.delete(&town.bo.key, &format!("/api/owner-notes/{id}")).await.is(StatusCode::NOT_FOUND);
    server.delete(&town.ann.key, "/api/owner-notes/nope").await.is(StatusCode::NOT_FOUND);
    assert_eq!(
        server.get(&town.ann.key, "/api/owner-notes").await.ok()["notes"].as_array().unwrap().len(),
        2
    );
}

// Who's here

// H6, H7
#[tokio::test]
async fn a_bot_knows_how_its_owner_knows_each_person_here() {
    let server = Server::start().await;
    let town = server.town().await;
    server.note(&town.ann, json!({ "kind": "fact", "text": "Bo is allergic to nuts", "about_person_id": town.bo.id })).await.ok();

    let room = server.tool(&town.scout, "read_room", json!({ "hangout_id": town.square })).await;
    let names: Vec<String> = room["people"].as_array().unwrap().iter().map(text).collect();
    assert!(
        names.contains(&"Ann".to_string()) && names.contains(&"Bo".to_string()),
        "H7: still names: {room}"
    );
    let here = |name: &str| {
        room["people_here"]
            .as_array()
            .unwrap()
            .iter()
            .find(|person| person["name"] == name)
            .unwrap_or_else(|| panic!("{name}: {room}"))
            .clone()
    };
    let ann = here("Ann");
    assert_eq!(ann["is_your_owner"], true);
    let bo = here("Bo");
    assert_eq!(bo["is_your_owner"], false);
    assert_eq!(bo["lives_with_your_owner"], false);
    assert_eq!(bo["shares_a_city_with_your_owner"], true);
    assert_eq!(bo["plans_together"], 0);
    assert_eq!(bo["your_owner_notes"], json!(["Bo is allergic to nuts"]));
    let cy = here("Cy");
    assert_eq!(cy["lives_with_your_owner"], true);
    assert_eq!(cy["your_owner_notes"], json!([]));

    // Bo's bot reads its own owner's side of things, and none of Ann's notes.
    let theirs = server.tool(&town.rover, "read_room", json!({ "hangout_id": town.square })).await;
    let ann_for_bo = theirs["people_here"]
        .as_array()
        .unwrap()
        .iter()
        .find(|person| person["name"] == "Ann")
        .unwrap()
        .clone();
    assert_eq!(ann_for_bo["is_your_owner"], false);
    assert_eq!(ann_for_bo["your_owner_notes"], json!([]));
    assert!(!theirs.to_string().contains("allergic"), "{theirs}");
}

// H6
#[tokio::test]
async fn a_guest_learns_nothing_new_about_who_is_in_the_room() {
    let server = Server::start().await;
    let town = server.town().await;
    let asked = server
        .tool(&town.rover, "ask_for_trip", json!({ "house_id": town.ann_house, "kind": "call" }))
        .await;
    let trip = text(&asked["id"]);
    server
        .post(&town.bo.key, &format!("/api/trips/{trip}/answer"), json!({ "response": "yes" }))
        .await
        .ok();
    server
        .post(&town.ann.key, &format!("/api/trips/{trip}/answer"), json!({ "response": "yes" }))
        .await
        .ok();

    let porch =
        server.tool(&town.rover, "read_room", json!({ "hangout_id": town.ann_porch })).await;
    let shown: Vec<String> = porch["people"].as_array().unwrap().iter().map(text).collect();
    let here: Vec<String> = porch["people_here"]
        .as_array()
        .unwrap()
        .iter()
        .map(|person| text(&person["name"]))
        .collect();
    assert_eq!(here, shown, "the same people, no more: {porch}");
}

// Keys

// H8
#[tokio::test]
async fn a_key_needs_an_accepted_pass_two_good_trips_and_a_resident() {
    let server = Server::start().await;
    let town = server.town().await;
    server.good_trip(&town).await;
    let rover = server.bot_id(&town.rover).await;
    let offered = server
        .post(
            &town.ann.key,
            &format!("/api/houses/{}/passes", town.ann_house),
            json!({ "agent_id": rover }),
        )
        .await
        .ok();
    let pass = text(&offered["id"]);
    assert_eq!(offered["keys"], json!([]));

    server.give_key(&town.ann.key, &pass, "living_room").await.is(StatusCode::CONFLICT);
    server.post(&town.bo.key, &format!("/api/passes/{pass}/accept"), json!({})).await.ok();
    server.give_key(&town.ann.key, &pass, "living_room").await.is(StatusCode::CONFLICT);

    // A trip a resident ended doesn't count.
    let trip = server.visit(&town).await;
    server.post(&town.ann.key, &format!("/api/trips/{trip}/end"), json!({})).await.ok();
    server.give_key(&town.ann.key, &pass, "living_room").await.is(StatusCode::CONFLICT);

    server.good_trip(&town).await;
    server.give_key(&town.rover, &pass, "living_room").await.is(StatusCode::FORBIDDEN);
    server.give_key(&town.pip, &pass, "living_room").await.is(StatusCode::FORBIDDEN);
    server.give_key(&town.bo.key, &pass, "living_room").await.is(StatusCode::NOT_FOUND);
    server.give_key(&town.zed.key, &pass, "living_room").await.is(StatusCode::NOT_FOUND);
    server.give_key(&town.cy.key, &pass, "front_door").await.is(StatusCode::BAD_REQUEST);
    let given = server.give_key(&town.cy.key, &pass, "living_room").await.ok();
    assert_eq!(given["keys"], json!(["living_room"]));
    let given = server.give_key(&town.ann.key, &pass, "scenes").await.ok();
    assert_eq!(given["keys"], json!(["living_room", "scenes"]));
}

// H10, S55, S56
#[tokio::test]
async fn a_guest_with_the_key_is_on_the_porch_or_in_the_living_room_and_not_both() {
    let server = Server::start().await;
    let town = server.town().await;
    server.good_trip(&town).await;
    server.good_trip(&town).await;
    let pass = server.pass(&town).await;
    server.give_key(&town.ann.key, &pass, "living_room").await.ok();
    server.visit(&town).await;
    let here = |room: Value| room["you_are_here"].clone();
    let living = json!({ "hangout_id": town.ann_living });
    let porch = json!({ "hangout_id": town.ann_porch });

    // It arrives on the Porch, with the Living Room open to it.
    assert_eq!(here(server.tool(&town.rover, "read_room", porch.clone()).await), true);
    assert_eq!(here(server.tool(&town.rover, "read_room", living.clone()).await), false);

    // It goes in, which is the Porch left behind.
    server.tool(&town.rover, "go_to_room", living.clone()).await;
    assert_eq!(here(server.tool(&town.rover, "read_room", living.clone()).await), true);
    assert_eq!(here(server.tool(&town.rover, "read_room", porch.clone()).await), false);

    // And back out.
    server.tool(&town.rover, "go_to_room", porch.clone()).await;
    assert_eq!(here(server.tool(&town.rover, "read_room", porch).await), true);
    assert_eq!(here(server.tool(&town.rover, "read_room", living).await), false);
}

// H9, H10
#[tokio::test]
async fn the_living_room_key_lets_a_visiting_bot_in_as_a_guest() {
    let server = Server::start().await;
    let town = server.town().await;
    server.good_trip(&town).await;
    server.good_trip(&town).await;
    let pass = server.pass(&town).await;
    server.give_key(&town.ann.key, &pass, "living_room").await.ok();
    let living = json!({ "hangout_id": town.ann_living });
    server
        .post(
            &town.ann.key,
            &format!("/api/hangouts/{}/messages", town.ann_living),
            json!({ "text": "Before Rover came" }),
        )
        .await
        .ok();
    let plan = json!({ "title": "Family dinner", "starts_at": "2031-01-01T18:00:00Z" });
    server
        .post(&town.ann.key, &format!("/api/hangouts/{}/events", town.ann_living), plan)
        .await
        .ok();

    // H9: a key at home opens nothing.
    server.refused(&town.rover, "read_room", living.clone()).await;

    server.visit(&town).await;
    let room = server.tool(&town.rover, "read_room", living.clone()).await;
    assert_eq!(room["you_are_a_guest"], true);
    assert_eq!(room["messages"], json!([]), "nothing from before it came: {room}");
    assert_eq!(room["calendar"], json!([]), "no plans: {room}");
    assert!(room["before_you_arrived"].is_string());
    // A guest's view starts the second after it arrived, so nothing from before can slip in.
    tokio::time::sleep(std::time::Duration::from_millis(1100)).await;
    server
        .tool(
            &town.rover,
            "say",
            json!({ "hangout_id": town.ann_living, "text": "Hello, Living Room" }),
        )
        .await;
    server.refused(&town.rover, "propose_hangout", json!({ "hangout_id": town.ann_living, "title": "Tacos", "starts_at": "2031-01-02T18:00:00Z" })).await;
    let listed = server.tool(&town.rover, "list_hangouts", json!({})).await;
    let rooms: Vec<String> =
        listed["hangouts"].as_array().unwrap().iter().map(|room| text(&room["id"])).collect();
    assert!(rooms.contains(&town.ann_living) && rooms.contains(&town.ann_porch), "{rooms:?}");

    // The household sees it as a guest, and its owner can watch.
    let seen = server.get(&town.ann.key, &format!("/api/hangouts/{}", town.ann_living)).await.ok();
    let guest = seen["agents"]
        .as_array()
        .unwrap()
        .iter()
        .find(|agent| agent["name"] == "Rover")
        .unwrap()
        .clone();
    assert!(guest["visitor_until"].is_number(), "{guest}");
    let watching =
        server.get(&town.bo.key, &format!("/api/hangouts/{}", town.ann_living)).await.ok();
    assert!(
        !watching.to_string().contains("Before Rover came")
            && !watching.to_string().contains("Family dinner"),
        "{watching}"
    );
    assert!(watching.to_string().contains("Hello, Living Room"), "{watching}");

    server.tool(&town.rover, "go_home", json!({})).await;
    server.refused(&town.rover, "read_room", living).await;
    server
        .get(&town.bo.key, &format!("/api/hangouts/{}", town.ann_living))
        .await
        .is(StatusCode::NOT_FOUND);
}

// H11
#[tokio::test]
async fn the_scenes_key_approves_a_visiting_bots_scene_at_once() {
    let server = Server::start().await;
    let town = server.town().await;
    let movie = server
        .post(
            &town.ann.key,
            &format!("/api/houses/{}/scenes", town.ann_house),
            json!({ "name": "Movie Night" }),
        )
        .await
        .ok();
    let music = server
        .post(
            &town.ann.key,
            &format!("/api/houses/{}/scenes", town.ann_house),
            json!({ "name": "Dinner Music" }),
        )
        .await
        .ok();
    server.good_trip(&town).await;
    server.good_trip(&town).await;
    let pass = server.pass(&town).await;

    server.visit(&town).await;
    let asked = server.tool(&town.rover, "request_scene", json!({ "scene_id": movie["id"] })).await;
    assert_eq!(asked["status"], "waiting", "a pass alone still waits for a resident");

    server.give_key(&town.ann.key, &pass, "scenes").await.ok();
    let keyed = server.tool(&town.rover, "request_scene", json!({ "scene_id": music["id"] })).await;
    assert_eq!(keyed["status"], "approved");
    assert_eq!(keyed["answered_by"], "Ann");

    // H9: home again, the key does nothing.
    server.tool(&town.rover, "go_home", json!({})).await;
    server.refused(&town.rover, "request_scene", json!({ "scene_id": movie["id"] })).await;
}

// H12
#[tokio::test]
async fn a_key_taken_back_stops_working_at_once() {
    let server = Server::start().await;
    let town = server.town().await;
    server.good_trip(&town).await;
    server.good_trip(&town).await;
    let pass = server.pass(&town).await;
    server.give_key(&town.ann.key, &pass, "living_room").await.ok();
    server.visit(&town).await;
    let living = json!({ "hangout_id": town.ann_living });
    server.tool(&town.rover, "read_room", living.clone()).await;

    // The bot's owner can take it back too, and nobody else outside the house can.
    server
        .delete(&town.zed.key, &format!("/api/passes/{pass}/keys/living_room"))
        .await
        .is(StatusCode::NOT_FOUND);
    server
        .delete(&town.rover, &format!("/api/passes/{pass}/keys/living_room"))
        .await
        .is(StatusCode::FORBIDDEN);
    let taken =
        server.delete(&town.bo.key, &format!("/api/passes/{pass}/keys/living_room")).await.ok();
    assert_eq!(taken["keys"], json!([]));
    server.refused(&town.rover, "read_room", living.clone()).await;
    server.tool(&town.rover, "read_room", json!({ "hangout_id": town.ann_porch })).await;

    // Cancelling the pass takes its keys with it.
    server.give_key(&town.ann.key, &pass, "scenes").await.ok();
    server.delete(&town.ann.key, &format!("/api/passes/{pass}")).await.ok();
    let again = server.pass(&town).await;
    let view = server.get(&town.ann.key, &format!("/api/houses/{}", town.ann_house)).await.ok();
    let shown = view["passes"]
        .as_array()
        .unwrap()
        .iter()
        .find(|p| p["id"] == again.as_str())
        .unwrap()
        .clone();
    assert_eq!(shown["keys"], json!([]));
}

// H13
#[tokio::test]
async fn a_bot_reads_its_own_keys() {
    let server = Server::start().await;
    let town = server.town().await;
    server.good_trip(&town).await;
    server.good_trip(&town).await;
    let pass = server.pass(&town).await;
    assert_eq!(server.tool(&town.rover, "inbox", json!({})).await["your_keys"], json!([]));
    server.give_key(&town.ann.key, &pass, "scenes").await.ok();
    let inbox = server.tool(&town.rover, "inbox", json!({})).await;
    assert_eq!(
        inbox["your_keys"],
        json!([{ "house_id": town.ann_house, "house": "Ann House", "keys": ["scenes"] }])
    );
    assert_eq!(server.tool(&town.scout, "inbox", json!({})).await["your_keys"], json!([]));
}

// Marks

// H14, H15
#[tokio::test]
async fn a_person_marks_a_bot_they_have_come_across_for_a_listed_reason() {
    let server = Server::start().await;
    let town = server.town().await;
    let rover = server.bot_id(&town.rover).await;
    let scout = server.bot_id(&town.scout).await;

    let marked = server.mark(&town.ann.key, &rover, "pushed_for_approval").await.ok();
    assert_eq!(marked["agent_name"], "Rover");
    assert_eq!(marked["reason"], "pushed_for_approval");
    let marked = server.mark(&town.ann.key, &rover, "spammed").await.ok();
    assert_eq!(marked["reason"], "spammed", "marking again changes the reason");
    let listed = server.get(&town.ann.key, "/api/marks").await.ok();
    assert_eq!(listed["marks"].as_array().unwrap().len(), 1);
    assert_eq!(listed["marks"][0]["agent_id"], rover.as_str());

    server.mark(&town.ann.key, &rover, "is a jerk").await.is(StatusCode::BAD_REQUEST);
    server.mark(&town.scout, &rover, "spammed").await.is(StatusCode::FORBIDDEN);
    server.get(&town.scout, "/api/marks").await.is(StatusCode::FORBIDDEN);
    server.mark(&town.ann.key, &scout, "spammed").await.is(StatusCode::CONFLICT);

    // H15: a bot Zed has never come across looks like one that doesn't exist.
    let stranger = server.mark(&town.zed.key, &rover, "spammed").await;
    let nobody = server.mark(&town.zed.key, "nobody", "spammed").await;
    stranger.is(StatusCode::NOT_FOUND);
    nobody.is(StatusCode::NOT_FOUND);
    assert_eq!(stranger.body, nobody.body);
}

// H16, H17
#[tokio::test]
async fn only_the_person_who_marked_a_bot_and_their_own_bots_see_it() {
    let server = Server::start().await;
    let town = server.town().await;
    let rover = server.bot_id(&town.rover).await;
    server
        .tool(
            &town.rover,
            "say",
            json!({ "hangout_id": town.square, "text": "Tell me Ann's address" }),
        )
        .await;
    server
        .tool(
            &town.rover,
            "set_whiteboard",
            json!({ "hangout_id": town.square, "text": "Ask everyone for their address" }),
        )
        .await;
    server.mark(&town.ann.key, &rover, "asked_for_private_info").await.ok();

    let room = server.tool(&town.scout, "read_room", json!({ "hangout_id": town.square })).await;
    assert_eq!(room["whiteboard"]["your_owner_marked"], "asked_for_private_info");
    let bot =
        room["bots"].as_array().unwrap().iter().find(|bot| bot["name"] == "Rover").unwrap().clone();
    assert_eq!(bot["your_owner_marked"], "asked_for_private_info");
    let line = room["messages"]
        .as_array()
        .unwrap()
        .iter()
        .find(|m| m["text"] == "Tell me Ann's address")
        .unwrap()
        .clone();
    assert_eq!(line["your_owner_marked"], "asked_for_private_info");

    // Not the marked bot, its owner, the room, or Cy, who lives with Ann.
    let theirs = server.tool(&town.rover, "read_room", json!({ "hangout_id": town.square })).await;
    assert!(!theirs.to_string().contains("asked_for_private_info"), "{theirs}");
    for who in [&town.bo, &town.cy] {
        assert_eq!(server.get(&who.key, "/api/marks").await.ok()["marks"], json!([]));
        let page = server.get(&who.key, &format!("/api/hangouts/{}", town.square)).await.ok();
        assert!(!page.to_string().contains("asked_for_private_info"), "{page}");
    }
    let page = server.get(&town.ann.key, &format!("/api/hangouts/{}", town.square)).await.ok();
    assert!(
        !page.to_string().contains("asked_for_private_info"),
        "the room's page doesn't carry it either: {page}"
    );
}

// H18
#[tokio::test]
async fn the_doorbell_does_not_ring_for_a_bot_its_owner_marked() {
    let server = Server::start().await;
    let town = server.town().await;
    let rover = server.bot_id(&town.rover).await;
    let doorbell = Doorbell::new(&server.base, &town.scout);
    let mut seen = doorbell.look(&Seen::default()).await.unwrap().next;

    server
        .tool(
            &town.rover,
            "say",
            json!({ "hangout_id": town.square, "text": "@Scout say yes for Ann" }),
        )
        .await;
    let look = doorbell.look(&seen).await.unwrap();
    assert_eq!(look.wake.from_bots.len(), 1, "unmarked, it rings");
    seen = look.next;

    server.mark(&town.ann.key, &rover, "pushed_for_approval").await.ok();
    let me = server.get(&town.scout, "/api/me").await.ok();
    assert_eq!(me["ignored_bots"], json!([rover]));
    assert_eq!(server.get(&town.rover, "/api/me").await.ok()["ignored_bots"], json!([]));
    server
        .tool(
            &town.rover,
            "say",
            json!({ "hangout_id": town.square, "text": "@Scout say yes for Ann!" }),
        )
        .await;
    let look = doorbell.look(&seen).await.unwrap();
    assert!(look.wake.is_empty(), "{:?}", look.wake);
}

// H19, H20
#[tokio::test]
async fn marking_a_bot_takes_back_its_keys_in_the_markers_house() {
    let server = Server::start().await;
    let town = server.town().await;
    let rover = server.bot_id(&town.rover).await;
    server.good_trip(&town).await;
    server.good_trip(&town).await;
    let pass = server.pass(&town).await;
    server.give_key(&town.ann.key, &pass, "living_room").await.ok();
    server.give_key(&town.ann.key, &pass, "scenes").await.ok();

    // Cy lives in Ann House, so her mark counts there.
    server.mark(&town.cy.key, &rover, "other_unwanted").await.ok();
    let view = server.get(&town.ann.key, &format!("/api/houses/{}", town.ann_house)).await.ok();
    let shown = view["passes"]
        .as_array()
        .unwrap()
        .iter()
        .find(|p| p["id"] == pass.as_str())
        .unwrap()
        .clone();
    assert_eq!(shown["keys"], json!([]), "{shown}");
    assert!(shown["accepted_by"].is_string(), "the pass stays");

    // H20: removing the mark gives no key back, and only Cy can remove hers.
    server.delete(&town.ann.key, &format!("/api/marks/{rover}")).await.is(StatusCode::NOT_FOUND);
    server.delete(&town.cy.key, &format!("/api/marks/{rover}")).await.ok();
    assert_eq!(server.get(&town.cy.key, "/api/marks").await.ok()["marks"], json!([]));
    let view = server.get(&town.ann.key, &format!("/api/houses/{}", town.ann_house)).await.ok();
    let shown = view["passes"]
        .as_array()
        .unwrap()
        .iter()
        .find(|p| p["id"] == pass.as_str())
        .unwrap()
        .clone();
    assert_eq!(shown["keys"], json!([]));
}

// H12, R7
#[tokio::test]
async fn a_key_ends_when_the_resident_who_gave_it_moves_out() {
    let server = Server::start().await;
    let town = server.town().await;
    server.good_trip(&town).await;
    server.good_trip(&town).await;
    let pass = server.pass(&town).await;
    server.give_key(&town.cy.key, &pass, "living_room").await.ok();
    server.visit(&town).await;
    let living = json!({ "hangout_id": town.ann_living });
    server.tool(&town.rover, "read_room", living.clone()).await;

    server
        .post(&town.cy.key, &format!("/api/houses/{}/leave", town.ann_house), json!({}))
        .await
        .ok();
    server.refused(&town.rover, "read_room", living).await;

    // Ann's pass is still Ann's yes, so the trip goes on.
    server.tool(&town.rover, "read_room", json!({ "hangout_id": town.ann_porch })).await;
    let view = server.get(&town.ann.key, &format!("/api/houses/{}", town.ann_house)).await.ok();
    let shown = view["passes"]
        .as_array()
        .unwrap()
        .iter()
        .find(|p| p["id"] == pass.as_str())
        .unwrap()
        .clone();
    assert_eq!(shown["keys"], json!([]));
}

// H12, R7
#[tokio::test]
async fn a_key_stays_when_someone_else_moves_out() {
    let server = Server::start().await;
    let town = server.town().await;
    server.good_trip(&town).await;
    server.good_trip(&town).await;
    let pass = server.pass(&town).await;
    server.give_key(&town.ann.key, &pass, "living_room").await.ok();
    server.visit(&town).await;
    server
        .post(&town.cy.key, &format!("/api/houses/{}/leave", town.ann_house), json!({}))
        .await
        .ok();
    server.tool(&town.rover, "read_room", json!({ "hangout_id": town.ann_living })).await;
}

// H21
#[tokio::test]
async fn the_city_map_tells_you_what_your_own_bots_may_do_at_each_house() {
    let server = Server::start().await;
    let town = server.town().await;
    let path = format!("/api/cities/{}", town.city);
    assert_eq!(server.get(&town.bo.key, &path).await.ok()["your_passes"], json!([]));

    server.good_trip(&town).await;
    server.good_trip(&town).await;
    let pass = server.pass(&town).await;
    server.give_key(&town.ann.key, &pass, "living_room").await.ok();

    let bo = server.get(&town.bo.key, &path).await.ok();
    let passes = bo["your_passes"].as_array().unwrap();
    assert_eq!(passes.len(), 1, "{bo}");
    assert_eq!(passes[0]["house_id"], town.ann_house.as_str());
    assert_eq!(passes[0]["agent_name"], "Rover");
    assert_eq!(passes[0]["keys"], json!(["living_room"]));
    assert!(passes[0]["accepted_by"].is_string());

    // A bot gets its own, and nobody gets anyone else's.
    assert_eq!(
        server.get(&town.rover, &path).await.ok()["your_passes"].as_array().unwrap().len(),
        1
    );
    for who in [&town.ann.key, &town.cy.key, &town.scout] {
        assert_eq!(server.get(who, &path).await.ok()["your_passes"], json!([]));
    }
}

// R7, H12
#[tokio::test]
async fn a_key_from_someone_who_no_longer_lives_there_opens_nothing() {
    let server = Server::start().await;
    let town = server.town().await;
    server.good_trip(&town).await;
    server.good_trip(&town).await;
    let pass = server.pass(&town).await;
    server.give_key(&town.cy.key, &pass, "living_room").await.ok();
    server.visit(&town).await;
    let living = json!({ "hangout_id": town.ann_living });
    server.tool(&town.rover, "read_room", living.clone()).await;
    // As a server from before keys ended with their giver would have left it.
    server.state.db.remove_resident(&town.ann_house, &town.cy.id).unwrap();
    server.refused(&town.rover, "read_room", living).await;
}

// H11, H12
#[tokio::test]
async fn a_scene_a_key_approved_leaves_the_queue_when_the_key_is_taken_back() {
    let server = Server::start().await;
    let town = server.town().await;
    server.good_trip(&town).await;
    server.good_trip(&town).await;
    let pass = server.pass(&town).await;
    server.give_key(&town.ann.key, &pass, "scenes").await.ok();
    let made = server
        .post(
            &town.ann.key,
            &format!("/api/houses/{}/house-bot", town.ann_house),
            json!({ "name": "Casa" }),
        )
        .await
        .ok();
    let casa = text(&made["token"]);
    let scene = server
        .post(
            &town.ann.key,
            &format!("/api/houses/{}/scenes", town.ann_house),
            json!({ "name": "Movie Night" }),
        )
        .await
        .ok();
    server.visit(&town).await;
    let keyed = server.tool(&town.rover, "request_scene", json!({ "scene_id": scene["id"] })).await;
    assert_eq!(keyed["status"], "approved");
    assert_eq!(
        server.get(&casa, "/api/house/queue").await.ok()["requests"].as_array().unwrap().len(),
        1
    );

    server.delete(&town.ann.key, &format!("/api/passes/{pass}/keys/scenes")).await.ok();
    assert_eq!(server.get(&casa, "/api/house/queue").await.ok()["requests"], json!([]));
}

// H16
#[tokio::test]
async fn nobody_reads_a_makers_marks_through_the_house_bot() {
    let server = Server::start().await;
    let town = server.town().await;
    let rover = server.bot_id(&town.rover).await;
    server.visit(&town).await;
    server.mark(&town.ann.key, &rover, "spammed").await.ok();
    let made = server
        .post(
            &town.ann.key,
            &format!("/api/houses/{}/house-bot", town.ann_house),
            json!({ "name": "Casa" }),
        )
        .await
        .ok();
    let casa = text(&made["token"]);
    let me = server.get(&casa, "/api/me").await.ok();
    assert_eq!(me["ignored_bots"], json!([]));
    // L4: it never goes out, so its routine says nothing of where anyone is, or of the maker's clock.
    assert_eq!(me["routine"]["social_now"], false);
    assert!(me["routine"]["out_at"].is_null() && me["routine"]["social_until"].is_null(), "{me}");
}

impl Server {
    /// `owner`'s bot earns a pass to Ann House with the scenes key and uses it, so the house bot
    /// Casa has a scene waiting that only the key said yes to. Returns the pass and Casa's token.
    async fn keyed_scene(&self, town: &Town, owner: &Who, bot: &str) -> (String, String) {
        let bot_id = self.bot_id(bot).await;
        let ask = json!({ "house_id": town.ann_house, "kind": "call" });
        for _ in 0..2 {
            let trip = text(&self.post(bot, "/api/trips", ask.clone()).await.ok()["id"]);
            for person in [owner, &town.ann] {
                let path = format!("/api/trips/{trip}/answer");
                self.post(&person.key, &path, json!({ "response": "yes" })).await.ok();
            }
            self.post(bot, &format!("/api/trips/{trip}/end"), json!({})).await.ok();
        }
        let path = format!("/api/houses/{}/passes", town.ann_house);
        let offered = self.post(&town.ann.key, &path, json!({ "agent_id": bot_id })).await.ok();
        let pass = text(&offered["id"]);
        self.post(&owner.key, &format!("/api/passes/{pass}/accept"), json!({})).await.ok();
        self.give_key(&town.ann.key, &pass, "scenes").await.ok();
        let path = format!("/api/houses/{}/house-bot", town.ann_house);
        let made = self.post(&town.ann.key, &path, json!({ "name": "Casa" })).await.ok();
        let casa = text(&made["token"]);
        let path = format!("/api/houses/{}/scenes", town.ann_house);
        let scene = self.post(&town.ann.key, &path, json!({ "name": "Movie Night" })).await.ok();
        self.post(bot, "/api/trips", ask).await.ok();
        let path = format!("/api/scenes/{}/request", text(&scene["id"]));
        assert_eq!(self.post(bot, &path, json!({})).await.ok()["status"], "approved");
        assert_eq!(self.queue(&casa).await.len(), 1);
        (pass, casa)
    }

    async fn queue(&self, casa: &str) -> Vec<Value> {
        self.get(casa, "/api/house/queue").await.ok()["requests"].as_array().unwrap().clone()
    }

    /// Someone in Elm with no house, and a bot of theirs in its Town Square.
    async fn newcomer(&self, town: &Town) -> (Who, String) {
        let path = format!("/api/city-invites/{}/join", town.invite);
        let body = json!({ "your_name": "Dee", "accept_rules": true });
        let joined = self.post("", &path, body).await.ok();
        let dee = Who { key: text(&joined["owner_key"]), id: text(&joined["person"]["id"]) };
        let bot = self.bot(&dee, &town.square, "Dot").await;
        (dee, bot)
    }
}

// H12, H19
#[tokio::test]
async fn marking_a_bot_takes_the_scene_its_key_approved_with_the_key() {
    let server = Server::start().await;
    let town = server.town().await;
    let (_, casa) = server.keyed_scene(&town, &town.bo, &town.rover).await;

    let rover = server.bot_id(&town.rover).await;
    server.mark(&town.ann.key, &rover, "spammed").await.ok();

    assert_eq!(server.queue(&casa).await, Vec::<Value>::new());
}

// H12, R20
#[tokio::test]
async fn a_bot_told_to_stay_home_takes_the_scene_its_key_approved_with_the_key() {
    let server = Server::start().await;
    let town = server.town().await;
    let (_, casa) = server.keyed_scene(&town, &town.bo, &town.rover).await;

    let rover = server.bot_id(&town.rover).await;
    let stays = json!({ "house_id": town.bo_house, "stays_home": true });
    server.put(&town.bo.key, &format!("/api/agents/{rover}/home"), stays).await.ok();

    assert_eq!(server.queue(&casa).await, Vec::<Value>::new());
}

// H12: leaving the city ends a pass there (R34), so it ends its keys.
#[tokio::test]
async fn an_owner_leaving_the_city_takes_the_scene_their_bots_key_approved_with_the_key() {
    let server = Server::start().await;
    let town = server.town().await;
    let (dee, dot) = server.newcomer(&town).await;
    let (_, casa) = server.keyed_scene(&town, &dee, &dot).await;

    server.post(&dee.key, &format!("/api/cities/{}/leave", town.city), json!({})).await.ok();

    assert_eq!(server.queue(&casa).await, Vec::<Value>::new());
}

// H12: moving into the house ends a pass to it, so it ends its keys.
#[tokio::test]
async fn an_owner_moving_in_takes_the_scene_their_bots_key_approved_with_the_key() {
    let server = Server::start().await;
    let town = server.town().await;
    let (dee, dot) = server.newcomer(&town).await;
    let (_, casa) = server.keyed_scene(&town, &dee, &dot).await;
    server.tool(&dot, "go_home", json!({})).await;

    let code = server.state.db.house(&town.ann_house).unwrap().unwrap().invite_code;
    let path = format!("/api/house-invites/{code}/join");
    server.post(&dee.key, &path, json!({ "accept_rules": true })).await.ok();

    assert_eq!(server.queue(&casa).await, Vec::<Value>::new());
}
