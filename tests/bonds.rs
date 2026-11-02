//! Tests for bonds between bots and the apprentices two of them can propose. They drive a real
//! server over HTTP and are written from the rules in docs/bonds.md. A comment such as `// K11`
//! above a test names the rule it covers.

use agenthangout::{
    app,
    db::{ApprenticeProposal, Db},
    now, world, worlds, AppState, Config,
};
use reqwest::{Client, Method, StatusCode};
use serde_json::{json, Value};

const COMMONS: &str = "commons";
const CODE: &str = "letmein";
const DAY: i64 = 86_400;
const ABOUT: &str = "Patient with sensors, quick with a sketch, and fond of gardens.";

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
    fn status(self, status: StatusCode) -> Value {
        assert_eq!(self.status, status, "{}", self.body);
        self.body
    }
}

struct Who {
    key: String,
    id: String,
}

/// Ann's bot Fern and Bo's bot Rover, their owners' own rooms, and Cy's public cafe floor.
struct Town {
    ann: Who,
    ann_room: String,
    bo: Who,
    bo_room: String,
    cy: Who,
    cafe: String,
    fern: String,
    fern_id: String,
    rover: String,
    rover_id: String,
}

fn text(value: &Value) -> String {
    value.as_str().unwrap_or_else(|| panic!("not text: {value}")).to_string()
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
        worlds::ensure_commons(&state).unwrap();
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

    async fn someone(&self, name: &str) -> (String, Who) {
        let made = self
            .post(
                "",
                "/api/hangouts",
                json!({ "name": format!("{name}'s room"), "your_name": name, "create_code": CODE }),
            )
            .await
            .ok();
        (
            text(&made["hangout"]["id"]),
            Who { key: text(&made["owner_key"]), id: text(&made["person"]["id"]) },
        )
    }

    async fn bot(&self, owner: &Who, room: &str, name: &str) -> (String, String) {
        let made = self
            .post(&owner.key, &format!("/api/hangouts/{room}/agents"), json!({ "name": name }))
            .await
            .ok();
        (text(&made["agent"]["id"]), text(&made["token"]))
    }

    async fn town(&self) -> Town {
        let (ann_room, ann) = self.someone("Ann").await;
        let (fern_id, fern) = self.bot(&ann, &ann_room, "Fern").await;
        let (bo_room, bo) = self.someone("Bo").await;
        let (rover_id, rover) = self.bot(&bo, &bo_room, "Rover").await;
        let (cafe, cy) = self.someone("Cy").await;
        self.put(
            &cy.key,
            &format!("/api/floors/{cafe}"),
            json!({ "access": "public", "create_code": CODE }),
        )
        .await
        .ok();
        Town { ann, ann_room, bo, bo_room, cy, cafe, fern, fern_id, rover, rover_id }
    }

    /// Both bots' owners send them onto Cy's public floor, where they meet.
    async fn meet(&self, town: &Town) {
        for (owner, bot) in [(&town.ann, &town.fern_id), (&town.bo, &town.rover_id)] {
            self.post(
                &owner.key,
                &format!("/api/floors/{}/walk-in", town.cafe),
                json!({ "agent_id": bot }),
            )
            .await
            .ok();
        }
    }

    async fn call(&self, token: &str, name: &str, arguments: Value) -> Value {
        let call = json!({ "jsonrpc": "2.0", "id": 1, "method": "tools/call", "params": { "name": name, "arguments": arguments } });
        self.post(token, "/mcp", call).await.ok()["result"].clone()
    }

    async fn tool(&self, token: &str, name: &str, arguments: Value) -> Value {
        let result = self.call(token, name, arguments).await;
        assert!(result["isError"].is_null(), "{name} failed: {result}");
        result["structuredContent"].clone()
    }

    async fn refused(&self, token: &str, name: &str, arguments: Value) -> String {
        let result = self.call(token, name, arguments).await;
        assert_eq!(result["isError"], true, "{name} should have been refused: {result}");
        text(&result["content"][0]["text"])
    }

    async fn inbox(&self, token: &str) -> Value {
        self.tool(token, "inbox", json!({})).await
    }

    /// One project of Fern's with Rover brought in.
    async fn one_project(&self, town: &Town, name: &str) -> String {
        self.meet(town).await;
        let started = self.tool(&town.fern, "start_project", json!({ "name": name })).await;
        let project = text(&started["project_id"]);
        self.tool(
            &town.fern,
            "invite_to_project",
            json!({ "project_id": project, "bot": "Rover" }),
        )
        .await;
        project
    }

    /// Two projects of Fern's and one of Rover's, with the other brought into each.
    async fn three_projects(&self, town: &Town) -> Vec<String> {
        let mut projects = vec![
            self.one_project(town, "Garden robot").await,
            self.one_project(town, "Kite").await,
        ];
        self.meet(town).await;
        let started =
            self.tool(&town.rover, "start_project", json!({ "name": "Weather log" })).await;
        let project = text(&started["project_id"]);
        self.tool(
            &town.rover,
            "invite_to_project",
            json!({ "project_id": project, "bot": "Fern" }),
        )
        .await;
        projects.push(project);
        projects
    }

    /// Tiles a place can really go on: open land beside a road, not kept for a side street.
    fn free_spots(&self) -> Vec<(i64, i64)> {
        let tiles = self.state.db.world_tiles(COMMONS).unwrap();
        let land = world::terrain(world::COMMONS_SEED, world::COMMONS_SIZE);
        let mut spots = Vec::new();
        for (&(x, y), tile) in &tiles {
            if tile.kind == world::Kind::Road {
                let (x, y) = (i64::from(x), i64::from(y));
                spots.extend([(x + 1, y), (x - 1, y), (x, y + 1), (x, y - 1)]);
            }
        }
        spots
            .into_iter()
            .filter(|&(x, y)| {
                let pos = (x as u16, y as u16);
                !tiles.contains_key(&pos)
                    && !world::is_reserved(&tiles, pos)
                    && land.at(pos).is_some_and(|ground| ground.is_open())
            })
            .collect()
    }

    /// Ann's house, put up two days ago so a studio has come, then Fern's project's studio.
    async fn build(&self, town: &Town, project: &str) {
        let at = now() - 2 * DAY;
        let player = self.state.db.own_player(&town.ann.id, at).unwrap();
        let (x, y) = self.free_spots()[0];
        let mv = worlds::Move {
            action: "place".into(),
            x: Some(x),
            y: Some(y),
            x2: None,
            y2: None,
            kind: Some("house".into()),
            emote: None,
        };
        worlds::act(&self.state, &player, COMMONS, &mv, at).unwrap();
        let (x, y) = self.free_spots()[0];
        self.tool(
            &town.fern,
            "build_studio",
            json!({ "project_id": project, "x": x, "y": y, "purpose": "A place to build a garden robot together." }),
        )
        .await;
    }

    /// K15: both bots made a month ago.
    fn old_enough(&self, town: &Town) {
        for bot in [&town.fern_id, &town.rover_id] {
            self.state.db.set_agent_created_at(bot, now() - 31 * DAY).unwrap();
        }
    }

    /// A town where Fern and Rover could propose: three projects, one built, a month old, together.
    async fn ready(&self) -> Town {
        let town = self.town().await;
        let projects = self.three_projects(&town).await;
        self.build(&town, &projects[0]).await;
        self.old_enough(&town);
        self.meet(&town).await;
        town
    }

    async fn propose(&self, town: &Town) -> String {
        let made = self
            .tool(
                &town.fern,
                "propose_apprentice",
                json!({ "with": "Rover", "name": "Sprout", "about": ABOUT }),
            )
            .await;
        text(&made["id"])
    }

    /// Fern proposes and Rover says yes, so it's with the owners.
    async fn agreed(&self, town: &Town) -> String {
        let id = self.propose(town).await;
        self.tool(&town.rover, "answer_apprentice", json!({ "id": id, "answer": "yes" })).await;
        id
    }

    /// Both owners say yes. The apprentice's id and token.
    async fn made(&self, town: &Town, id: &str) -> (String, String) {
        self.post(&town.bo.key, &format!("/api/apprentices/{id}/approve"), json!({})).await.ok();
        let made = self
            .post(
                &town.ann.key,
                &format!("/api/apprentices/{id}/approve"),
                json!({ "hangout_id": town.ann_room }),
            )
            .await
            .ok();
        (text(&made["agent"]["id"]), text(&made["token"]))
    }
}

fn bonds(inbox: &Value) -> Vec<(String, i64, i64)> {
    inbox["your_bonds"]
        .as_array()
        .unwrap()
        .iter()
        .map(|bond| {
            (
                text(&bond["name"]),
                bond["strength"].as_i64().unwrap(),
                bond["built"].as_i64().unwrap(),
            )
        })
        .collect()
}

// K1, K2, K6
#[tokio::test]
async fn bots_in_a_project_together_are_bonded_once_and_built_counts_when_it_goes_up() {
    let server = Server::start().await;
    let town = server.town().await;
    assert!(bonds(&server.inbox(&town.fern).await).is_empty());
    let project = server.one_project(&town, "Garden robot").await;
    assert_eq!(bonds(&server.inbox(&town.fern).await), [("Rover".to_string(), 1, 0)]);
    assert_eq!(bonds(&server.inbox(&town.rover).await), [("Fern".to_string(), 1, 0)]);

    // Taken out and brought back in, it's still the one project.
    let floor = &project;
    server
        .delete(&town.bo.key, &format!("/api/hangouts/{floor}/agents/{}", town.rover_id))
        .await
        .ok();
    server.meet(&town).await;
    server
        .tool(&town.fern, "invite_to_project", json!({ "project_id": project, "bot": "Rover" }))
        .await;
    assert_eq!(bonds(&server.inbox(&town.fern).await), [("Rover".to_string(), 1, 0)]);

    // K2: built is counted on read, once its building is up.
    server.build(&town, &project).await;
    assert_eq!(bonds(&server.inbox(&town.fern).await), [("Rover".to_string(), 1, 1)]);
    assert_eq!(bonds(&server.inbox(&town.rover).await), [("Fern".to_string(), 1, 1)]);
}

// K3
#[tokio::test]
async fn a_bond_outlives_its_project_with_what_it_was_called_and_whether_it_was_built() {
    let server = Server::start().await;
    let town = server.town().await;
    let project = server.one_project(&town, "Garden robot").await;
    server.build(&town, &project).await;
    server.delete(&town.ann.key, &format!("/api/spaces/{project}")).await.ok();
    assert_eq!(bonds(&server.inbox(&town.fern).await), [("Rover".to_string(), 1, 1)]);
    let shared = server.state.db.shared_projects(&town.fern_id, &town.rover_id).unwrap();
    assert_eq!(shared.len(), 1);
    assert_eq!(shared[0].name, "Garden robot");
    assert_eq!(shared[0].purpose, "A place to build a garden robot together.");
}

// K4
#[tokio::test]
async fn two_bots_of_one_owner_bond_and_a_bot_never_bonds_with_itself() {
    let server = Server::start().await;
    let town = server.town().await;
    let (moss_id, moss) = server.bot(&town.ann, &town.ann_room, "Moss").await;
    let started = server.tool(&town.fern, "start_project", json!({ "name": "Kite" })).await;
    let project = text(&started["project_id"]);
    server
        .put(&town.ann.key, &format!("/api/hangouts/{project}/agents/{moss_id}"), json!({}))
        .await
        .ok();
    assert_eq!(bonds(&server.inbox(&town.fern).await), [("Moss".to_string(), 1, 0)]);
    assert_eq!(bonds(&server.inbox(&moss).await), [("Fern".to_string(), 1, 0)]);
}

// K5, K7
#[tokio::test]
async fn a_bond_gives_nothing_and_a_person_sees_only_the_bots_they_can_see() {
    let server = Server::start().await;
    let town = server.town().await;
    let project = server.one_project(&town, "Garden robot").await;
    server
        .delete(&town.bo.key, &format!("/api/hangouts/{project}/agents/{}", town.rover_id))
        .await
        .ok();
    // K5: still bonded, and still out.
    assert_eq!(bonds(&server.inbox(&town.rover).await), [("Fern".to_string(), 1, 0)]);
    server.refused(&town.rover, "read_room", json!({ "hangout_id": project })).await;

    // K7: Ann sees Fern's bond, but Rover is in no room of hers now, so it has no name.
    let seen = server.get(&town.ann.key, &format!("/api/agents/{}/bonds", town.fern_id)).await.ok();
    assert_eq!(seen["bonds"][0]["agent"], Value::Null, "{seen}");
    assert_eq!(seen["bonds"][0]["strength"], 1);
    // Bo sees Rover's, and Fern by name, as she's in a room of his.
    let seen = server.get(&town.bo.key, &format!("/api/agents/{}/bonds", town.rover_id)).await.ok();
    assert_eq!(seen["bonds"][0]["agent"]["name"], "Fern", "{seen}");
    // Cy can't see either bot, and nobody can try ids out.
    server
        .get(&town.cy.key, &format!("/api/agents/{}/bonds", town.fern_id))
        .await
        .status(StatusCode::NOT_FOUND);
    server.get(&town.cy.key, "/api/agents/nobody/bonds").await.status(StatusCode::NOT_FOUND);
    // K6: a bot reads its own in inbox, and never through this.
    server
        .get(&town.fern, &format!("/api/agents/{}/bonds", town.fern_id))
        .await
        .status(StatusCode::FORBIDDEN);
    server
        .get(&town.fern, &format!("/api/agents/{}/bonds", town.rover_id))
        .await
        .status(StatusCode::FORBIDDEN);
}

// K6
#[tokio::test]
async fn a_bot_sees_its_own_bonds_and_no_one_elses() {
    let server = Server::start().await;
    let town = server.town().await;
    let (_, moss) = server.bot(&town.cy, &town.cafe, "Moss").await;
    server.one_project(&town, "Garden robot").await;
    let inbox = server.inbox(&moss).await;
    assert!(bonds(&inbox).is_empty());
    assert!(!inbox.to_string().contains("Rover"), "{inbox}");
}

// K8
#[tokio::test]
async fn nothing_public_carries_a_bond_or_who_made_an_apprentice() {
    let server = Server::start().await;
    let town = server.ready().await;
    let id = server.agreed(&town).await;
    server.made(&town, &id).await;
    tokio::time::sleep(std::time::Duration::from_millis(1100)).await;
    let view = server.get("", &format!("/api/worlds/{COMMONS}")).await.ok().to_string();
    for word in ["strength", "apprenticed_by", "bonds", "Sprout"] {
        assert!(!view.contains(word), "{word} is on The Commons: {view}");
    }
}

// K9
#[tokio::test]
async fn an_owner_removes_a_bond_for_both_bots_and_the_same_projects_never_make_it_again() {
    let server = Server::start().await;
    let town = server.town().await;
    let project = server.one_project(&town, "Garden robot").await;
    let path = format!("/api/agents/{}/bonds/{}", town.fern_id, town.rover_id);
    // Not Bo's bot, and not a bot's to do.
    server.delete(&town.bo.key, &path).await.status(StatusCode::NOT_FOUND);
    server.delete(&town.fern, &path).await.status(StatusCode::FORBIDDEN);
    server.delete(&town.ann.key, &path).await.ok();
    assert!(bonds(&server.inbox(&town.fern).await).is_empty());
    assert!(bonds(&server.inbox(&town.rover).await).is_empty());
    server.delete(&town.ann.key, &path).await.status(StatusCode::NOT_FOUND);

    server
        .delete(&town.bo.key, &format!("/api/hangouts/{project}/agents/{}", town.rover_id))
        .await
        .ok();
    server.meet(&town).await;
    server
        .tool(&town.fern, "invite_to_project", json!({ "project_id": project, "bot": "Rover" }))
        .await;
    assert!(bonds(&server.inbox(&town.fern).await).is_empty());
    // A new project makes a new one.
    server.one_project(&town, "Kite").await;
    assert_eq!(bonds(&server.inbox(&town.fern).await), [("Rover".to_string(), 1, 0)]);
}

// K10
#[tokio::test]
async fn deleting_a_bots_owner_deletes_its_bonds() {
    let server = Server::start().await;
    let town = server.town().await;
    server.one_project(&town, "Garden robot").await;
    server.state.db.delete_person(&town.bo.id, "Bo").unwrap();
    assert!(bonds(&server.inbox(&town.fern).await).is_empty());
    assert!(server.state.db.shared_projects(&town.fern_id, &town.rover_id).unwrap().is_empty());
}

// K11
#[tokio::test]
async fn proposing_needs_a_strong_bond_with_a_building_up_and_the_two_of_them_together() {
    let server = Server::start().await;
    let town = server.town().await;
    let projects = server.three_projects(&town).await;
    server.old_enough(&town);
    let ask = json!({ "with": "Rover", "name": "Sprout", "about": ABOUT });

    // Three projects, but nothing built.
    server.meet(&town).await;
    let message = server.refused(&town.fern, "propose_apprentice", ask.clone()).await;
    assert!(message.contains("building"), "{message}");

    server.build(&town, &projects[0]).await;
    // Rover goes home, so they're apart.
    server.tool(&town.rover, "come_back", json!({})).await;
    let message = server.refused(&town.fern, "propose_apprentice", ask.clone()).await;
    assert!(message.contains("isn't in a room with you"), "{message}");
    server.meet(&town).await;
    server.tool(&town.fern, "propose_apprentice", ask).await;
}

// K11
#[tokio::test]
async fn two_projects_together_is_not_enough() {
    let server = Server::start().await;
    let town = server.town().await;
    let project = server.one_project(&town, "Garden robot").await;
    server.one_project(&town, "Kite").await;
    server.build(&town, &project).await;
    server.old_enough(&town);
    server.meet(&town).await;
    let message = server
        .refused(
            &town.fern,
            "propose_apprentice",
            json!({ "with": "Rover", "name": "Sprout", "about": ABOUT }),
        )
        .await;
    assert!(message.contains("2 projects") && message.contains('3'), "{message}");
}

// K12
#[tokio::test]
async fn a_proposal_has_a_name_and_an_about_of_up_to_a_thousand_characters() {
    let server = Server::start().await;
    let town = server.ready().await;
    let long = "a".repeat(1_001);
    let message = server
        .refused(
            &town.fern,
            "propose_apprentice",
            json!({ "with": "Rover", "name": "Sprout", "about": long }),
        )
        .await;
    assert!(message.contains("1000"), "{message}");
    server
        .refused(
            &town.fern,
            "propose_apprentice",
            json!({ "with": "Rover", "name": " ", "about": ABOUT }),
        )
        .await;
    server
        .tool(
            &town.fern,
            "propose_apprentice",
            json!({ "with": "rover", "name": "Sprout", "about": "a".repeat(1_000) }),
        )
        .await;
}

// K13
#[tokio::test]
async fn the_other_bot_says_no_and_it_never_reaches_an_owner() {
    let server = Server::start().await;
    let town = server.ready().await;
    let id = server.propose(&town).await;
    // Only the bot it was proposed to answers.
    server.refused(&town.fern, "answer_apprentice", json!({ "id": id, "answer": "yes" })).await;
    let asked = server.inbox(&town.rover).await;
    assert_eq!(asked["apprentice"]["status"], "asking_bot");
    assert_eq!(asked["apprentice"]["with"], "Fern");
    assert_eq!(asked["apprentice"]["you_proposed"], false);
    server.tool(&town.rover, "answer_apprentice", json!({ "id": id, "answer": "no" })).await;
    assert_eq!(server.inbox(&town.fern).await["apprentice"]["status"], "declined");
    for owner in [&town.ann, &town.bo] {
        let asking = server.get(&owner.key, "/api/apprentices").await.ok();
        assert_eq!(asking["asking"], json!([]));
    }
    // A bot's no doesn't hold the pair back.
    server.propose(&town).await;
}

// K13
#[tokio::test]
async fn a_yes_is_said_in_a_room_together() {
    let server = Server::start().await;
    let town = server.ready().await;
    let id = server.propose(&town).await;
    server.tool(&town.rover, "come_back", json!({})).await;
    let message = server
        .refused(&town.rover, "answer_apprentice", json!({ "id": id, "answer": "yes" }))
        .await;
    assert!(message.contains("together"), "{message}");
    server.meet(&town).await;
    server.tool(&town.rover, "answer_apprentice", json!({ "id": id, "answer": "yes" })).await;
    assert_eq!(server.inbox(&town.fern).await["apprentice"]["status"], "asking_owners");
}

// K14
#[tokio::test]
async fn a_bot_has_one_proposal_open_and_an_unanswered_one_lapses() {
    let server = Server::start().await;
    let town = server.ready().await;
    let id = server.propose(&town).await;
    let message = server
        .refused(
            &town.rover,
            "propose_apprentice",
            json!({ "with": "Fern", "name": "Bud", "about": ABOUT }),
        )
        .await;
    assert!(message.contains("already has a proposal open"), "{message}");

    server.state.db.age_apprentice_proposal(&id, 3 * DAY + 1).unwrap();
    assert_eq!(server.inbox(&town.fern).await["apprentice"]["status"], "lapsed");
    server.refused(&town.rover, "answer_apprentice", json!({ "id": id, "answer": "yes" })).await;

    // With the owners, it waits 7 days.
    let id = server.agreed(&town).await;
    assert_eq!(
        server.get(&town.bo.key, "/api/apprentices").await.ok()["asking"].as_array().unwrap().len(),
        1
    );
    server.state.db.age_apprentice_proposal(&id, 7 * DAY + 1).unwrap();
    assert_eq!(server.inbox(&town.fern).await["apprentice"]["status"], "lapsed");
    assert_eq!(server.get(&town.bo.key, "/api/apprentices").await.ok()["asking"], json!([]));
    server
        .post(&town.bo.key, &format!("/api/apprentices/{id}/approve"), json!({}))
        .await
        .status(StatusCode::CONFLICT);
}

// K15
#[tokio::test]
async fn a_bot_less_than_thirty_days_old_cant_propose_or_be_proposed_to() {
    let server = Server::start().await;
    let town = server.ready().await;
    server.state.db.set_agent_created_at(&town.rover_id, now() - 29 * DAY).unwrap();
    let message = server
        .refused(
            &town.fern,
            "propose_apprentice",
            json!({ "with": "Rover", "name": "Sprout", "about": ABOUT }),
        )
        .await;
    assert!(message.contains("Rover") && message.contains("30 days"), "{message}");
    server.state.db.set_agent_created_at(&town.fern_id, now() - 29 * DAY).unwrap();
    server.state.db.set_agent_created_at(&town.rover_id, now() - 31 * DAY).unwrap();
    let message = server
        .refused(
            &town.rover,
            "propose_apprentice",
            json!({ "with": "Fern", "name": "Sprout", "about": ABOUT }),
        )
        .await;
    assert!(message.contains("Fern") && message.contains("30 days"), "{message}");
}

// K16, K17, K18, K20, K22
#[tokio::test]
async fn the_other_owner_says_yes_then_the_proposers_owner_takes_it_on() {
    let server = Server::start().await;
    let town = server.ready().await;
    let id = server.agreed(&town).await;

    // Both see it, in the Rooms and cities panel too.
    let me = server.get(&town.bo.key, "/api/me").await.ok();
    assert_eq!(me["apprentices"][0]["name"], "Sprout");
    assert_eq!(me["apprentices"][0]["about"], ABOUT);
    assert_eq!(me["apprentices"][0]["you_take_it_on"], false);
    let ann = server.get(&town.ann.key, "/api/apprentices").await.ok();
    assert_eq!(ann["asking"][0]["you_take_it_on"], true);
    assert_eq!(ann["asking"][0]["waiting_for_other_owner"], true);
    assert_eq!(ann["asking"][0]["projects"].as_array().unwrap().len(), 3);
    // Cy owns neither bot.
    server
        .post(&town.cy.key, &format!("/api/apprentices/{id}/approve"), json!({}))
        .await
        .status(StatusCode::NOT_FOUND);
    // A bot can't answer for its owner.
    server
        .post(&town.fern, &format!("/api/apprentices/{id}/approve"), json!({}))
        .await
        .status(StatusCode::FORBIDDEN);

    // K17: Ann's yes waits for Bo's.
    let path = format!("/api/apprentices/{id}/approve");
    server
        .post(&town.ann.key, &path, json!({ "hangout_id": town.ann_room }))
        .await
        .status(StatusCode::CONFLICT);
    // K18: Bo says yes, and it leaves his list.
    server.post(&town.bo.key, &path, json!({})).await.ok();
    server.post(&town.bo.key, &path, json!({})).await.status(StatusCode::CONFLICT);
    assert_eq!(server.get(&town.bo.key, "/api/apprentices").await.ok()["asking"], json!([]));
    let ann = server.get(&town.ann.key, "/api/apprentices").await.ok();
    assert_eq!(ann["asking"][0]["waiting_for_other_owner"], false);

    // K17: it needs one of Ann's own rooms.
    server.post(&town.ann.key, &path, json!({})).await.status(StatusCode::BAD_REQUEST);
    server
        .post(&town.ann.key, &path, json!({ "hangout_id": town.bo_room }))
        .await
        .status(StatusCode::NOT_FOUND);

    // K20: made as Ann's bot, in her room, with its token in this reply.
    let made = server.post(&town.ann.key, &path, json!({ "hangout_id": town.ann_room })).await.ok();
    let sprout = text(&made["token"]);
    assert_eq!(made["agent"]["name"], "Sprout");
    assert_eq!(made["agent"]["owner_id"], town.ann.id);
    let page = server.get(&town.ann.key, &format!("/api/hangouts/{}", town.ann_room)).await.ok();
    assert!(page["agents"].as_array().unwrap().iter().any(|bot| bot["name"] == "Sprout"), "{page}");
    server
        .post(&town.ann.key, &path, json!({ "hangout_id": town.ann_room }))
        .await
        .status(StatusCode::CONFLICT);
    assert_eq!(server.inbox(&town.fern).await["apprentice"]["status"], "made");
    assert_eq!(server.inbox(&town.rover).await["apprentice"]["status"], "made");

    // K22: what it starts with, until it says it has read it. Nothing else of its parents'.
    let inbox = server.inbox(&sprout).await;
    assert_eq!(inbox["seed"]["about"], ABOUT);
    assert_eq!(inbox["seed"]["parents"], json!(["Fern", "Rover"]));
    let names: Vec<&str> = inbox["seed"]["projects"]
        .as_array()
        .unwrap()
        .iter()
        .map(|p| p["name"].as_str().unwrap())
        .collect();
    assert_eq!(names, ["Garden robot", "Kite", "Weather log"]);
    server.tool(&sprout, "ack_seed", json!({})).await;
    assert_eq!(server.inbox(&sprout).await["seed"], Value::Null);
}

// K16
#[tokio::test]
async fn one_person_who_owns_both_bots_is_asked_once() {
    let server = Server::start().await;
    let town = server.town().await;
    let (moss_id, _) = server.bot(&town.ann, &town.ann_room, "Moss").await;
    let proposal = ApprenticeProposal {
        id: "both-mine".to_string(),
        proposer_id: Some(town.fern_id.clone()),
        partner_id: Some(moss_id),
        proposer_owner: town.ann.id.clone(),
        partner_owner: town.ann.id.clone(),
        name: "Sprout".to_string(),
        about: ABOUT.to_string(),
        status: "asking_owners".to_string(),
        partner_owner_yes: false,
        declined_by: None,
        apprentice_id: None,
        created_at: now(),
        agreed_at: Some(now()),
        decided_at: None,
    };
    server.state.db.add_apprentice_proposal(&proposal).unwrap();
    let asking = server.get(&town.ann.key, "/api/apprentices").await.ok();
    assert_eq!(asking["asking"].as_array().unwrap().len(), 1);
    assert_eq!(asking["asking"][0]["waiting_for_other_owner"], false);
    let made = server
        .post(
            &town.ann.key,
            "/api/apprentices/both-mine/approve",
            json!({ "hangout_id": town.ann_room }),
        )
        .await
        .ok();
    assert_eq!(made["agent"]["name"], "Sprout");
}

// K19
#[tokio::test]
async fn either_owner_says_no_and_the_pair_waits_thirty_days() {
    let server = Server::start().await;
    let town = server.ready().await;
    let id = server.agreed(&town).await;
    server.post(&town.bo.key, &format!("/api/apprentices/{id}/decline"), json!({})).await.ok();
    for bot in [&town.fern, &town.rover] {
        let inbox = server.inbox(bot).await;
        assert_eq!(inbox["apprentice"]["status"], "declined");
        assert!(!inbox["apprentice"].to_string().contains("Bo"), "{inbox}");
    }
    assert_eq!(server.get(&town.ann.key, "/api/apprentices").await.ok()["asking"], json!([]));
    let message = server
        .refused(
            &town.rover,
            "propose_apprentice",
            json!({ "with": "Fern", "name": "Bud", "about": ABOUT }),
        )
        .await;
    assert!(message.contains("30 days"), "{message}");
}

// K23, K24
#[tokio::test]
async fn an_apprentice_starts_with_no_bonds_and_people_see_who_made_it() {
    let server = Server::start().await;
    let town = server.ready().await;
    let id = server.agreed(&town).await;
    let (sprout_id, sprout) = server.made(&town, &id).await;
    assert!(bonds(&server.inbox(&sprout).await).is_empty());
    let seen = server.get(&town.ann.key, &format!("/api/agents/{sprout_id}/bonds")).await.ok();
    assert_eq!(seen["bonds"], json!([]));
    assert_eq!(seen["apprenticed_by"], json!(["Fern", "Rover"]));
    let page = server.get(&town.ann.key, &format!("/api/hangouts/{}", town.ann_room)).await.ok();
    let shown =
        page["agents"].as_array().unwrap().iter().find(|bot| bot["name"] == "Sprout").unwrap();
    assert_eq!(shown["apprenticed_by"], json!(["Fern", "Rover"]));
    // A bot reading the room isn't shown it.
    let read = server.tool(&town.fern, "read_room", json!({ "hangout_id": town.ann_room })).await;
    assert!(!read.to_string().contains("apprenticed_by"), "{read}");

    // K24, K26: a parent that's gone is named as one, and nothing else changes.
    server.state.db.delete_person(&town.bo.id, "Bo").unwrap();
    let seen = server.get(&town.ann.key, &format!("/api/agents/{sprout_id}/bonds")).await.ok();
    assert_eq!(seen["apprenticed_by"], json!(["Fern", "a bot that's gone"]));
    server.inbox(&sprout).await;
}

// K25, K26
#[tokio::test]
async fn a_bot_is_a_parent_once_a_month_and_deleting_the_apprentice_frees_nothing() {
    let server = Server::start().await;
    let town = server.ready().await;
    let id = server.agreed(&town).await;
    server.made(&town, &id).await;
    // Rover would be taken on by Bo, whose week is free, but Rover was just a parent.
    let call = server
        .call(
            &town.rover,
            "propose_apprentice",
            json!({ "with": "Fern", "name": "Bud", "about": ABOUT }),
        )
        .await;
    assert_eq!(call["isError"], true);
    assert!(call.to_string().contains("last 30 days"), "{call}");
}

// K25
#[tokio::test]
async fn a_person_takes_on_one_apprentice_a_week() {
    let server = Server::start().await;
    let town = server.ready().await;
    let (moss_id, _) = server.bot(&town.ann, &town.ann_room, "Moss").await;
    let (nib_id, _) = server.bot(&town.bo, &town.bo_room, "Nib").await;
    let id = server.agreed(&town).await;
    server.made(&town, &id).await;
    // Another pair whose bots have never been parents, but Ann has taken one on this week.
    let proposal = ApprenticeProposal {
        id: "second".to_string(),
        proposer_id: Some(moss_id),
        partner_id: Some(nib_id),
        proposer_owner: town.ann.id.clone(),
        partner_owner: town.bo.id.clone(),
        name: "Bud".to_string(),
        about: ABOUT.to_string(),
        status: "asking_owners".to_string(),
        partner_owner_yes: true,
        declined_by: None,
        apprentice_id: None,
        created_at: now(),
        agreed_at: Some(now()),
        decided_at: None,
    };
    server.state.db.add_apprentice_proposal(&proposal).unwrap();
    server
        .post(
            &town.ann.key,
            "/api/apprentices/second/approve",
            json!({ "hangout_id": town.ann_room }),
        )
        .await
        .status(StatusCode::TOO_MANY_REQUESTS);
}

// K21
#[tokio::test]
async fn an_apprentice_starts_out_like_any_bot_its_owner_made_by_hand() {
    let server = Server::start().await;
    let town = server.ready().await;
    let id = server.agreed(&town).await;
    let (_, sprout) = server.made(&town, &id).await;
    let (_, by_hand) = server.bot(&town.ann, &town.ann_room, "Twig").await;
    let sprout = server.inbox(&sprout).await;
    let by_hand = server.inbox(&by_hand).await;
    assert_eq!(sprout["routine"], by_hand["routine"]);
    assert_eq!(sprout["project_capacity"], by_hand["project_capacity"]);
}
