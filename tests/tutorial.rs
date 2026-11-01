//! Tests for the tutorial. The ones about how far someone has got drive a real server over HTTP, and
//! the ones about the page read what the page says. They are written from the rules in
//! docs/tutorial.md. A comment such as `// G6` above a test names the rule it covers.

use std::sync::atomic::{AtomicUsize, Ordering};

use agenthangout::{app, db::Db, worlds, AppState, Config};
use reqwest::{Client, Method, StatusCode};
use serde_json::{json, Value};

const CODE: &str = "letmein";
const LATER: &str = "2031-06-06T18:00:00-05:00";

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
}

#[derive(Clone)]
struct Who {
    key: String,
    id: String,
}

fn text(value: &Value) -> String {
    value.as_str().unwrap_or_else(|| panic!("not text: {value}")).to_string()
}

fn code_of(url: &Value) -> String {
    text(url).rsplit('/').next().unwrap().to_string()
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

    /// A room started by someone new: its id, its link's code and who started it.
    async fn room(&self, name: &str) -> (String, String, Who) {
        let made = self
            .post(
                "",
                "/api/hangouts",
                json!({ "name": format!("{name}'s room"), "your_name": name, "create_code": CODE }),
            )
            .await
            .ok();
        (text(&made["hangout"]["id"]), code_of(&made["invite_url"]), who(&made))
    }

    /// Someone with nothing yet. Making an account here takes a room, so theirs is closed again.
    async fn someone(&self, name: &str) -> Who {
        let (room, _, person) = self.room(name).await;
        self.send(&person.key, Method::DELETE, &format!("/api/spaces/{room}"), None).await.ok();
        person
    }

    async fn join(&self, code: &str, name: &str) -> Who {
        who(&self
            .post("", &format!("/api/invites/{code}/join"), json!({ "your_name": name }))
            .await
            .ok())
    }

    async fn progress(&self, person: &Who) -> Value {
        self.get(&person.key, "/api/tutorial").await.ok()
    }

    /// Puts up a house on the first open tile beside a road, and says where.
    async fn house(&self, person: &Who) -> (i64, i64) {
        worlds::ensure_commons(&self.state).unwrap();
        let view = self.get("", "/api/worlds/commons").await.ok();
        for road in view["tiles"].as_array().unwrap().iter().filter(|tile| tile[2] == "road") {
            let (x, y) = (road[0].as_i64().unwrap(), road[1].as_i64().unwrap());
            for (x, y) in [(x + 1, y), (x - 1, y), (x, y + 1), (x, y - 1)] {
                let body = json!({ "action": "place", "kind": "house", "x": x, "y": y });
                let placed = self.post(&person.key, "/api/worlds/commons/act", body).await;
                if placed.status == StatusCode::OK {
                    return (x, y);
                }
            }
        }
        panic!("nowhere to build a house");
    }

    async fn clear(&self, person: &Who, (x, y): (i64, i64)) {
        let body = json!({ "action": "bulldoze", "x": x, "y": y });
        self.post(&person.key, "/api/worlds/commons/act", body).await.ok();
    }

    async fn bot(&self, owner: &Who, room: &str) -> String {
        let made = self
            .post(&owner.key, &format!("/api/hangouts/{room}/agents"), json!({ "name": "Scout" }))
            .await
            .ok();
        text(&made["token"])
    }

    async fn plan(&self, person: &Who, room: &str) -> String {
        let plan = json!({ "title": "Tacos", "starts_at": LATER, "min_yes": 2 });
        let made = self.post(&person.key, &format!("/api/hangouts/{room}/events"), plan).await.ok();
        text(&made["id"])
    }

    async fn answer(&self, person: &Who, event: &str, response: &str) {
        let body = json!({ "response": response });
        self.post(&person.key, &format!("/api/events/{event}/rsvp"), body).await.ok();
    }
}

/// The page's own script, written the same way on any machine.
fn page(name: &str) -> String {
    let source = match name {
        "app.js" => include_str!("../web/app.js"),
        "tutorial.js" => include_str!("../web/tutorial.js"),
        "style.css" => include_str!("../web/style.css"),
        _ => panic!("no such page: {name}"),
    };
    source.replace("\r\n", "\n")
}

/// Every name between two pairs of stars in the tutorial.
fn names_in_bold() -> Vec<String> {
    page("tutorial.js").split("**").skip(1).step_by(2).map(str::to_string).collect()
}

/// The part of the page from a line to the first line after it that starts with `end`.
fn part(source: &str, start: &str, end: &str) -> String {
    let at = source.find(start).unwrap_or_else(|| panic!("no {start}"));
    let rest = &source[at..];
    rest[..rest.find(end).unwrap_or_else(|| panic!("no {end} after {start}"))].to_string()
}

// G1
#[test]
fn the_tutorial_is_a_panel_on_the_map_and_in_a_room_with_a_button_at_every_width() {
    let app = page("app.js");
    assert!(app.contains("import { tutorial } from './tutorial.js';"));
    let tutorial = page("tutorial.js");
    assert!(tutorial.contains("panels.add(WORD, 'Tutorial',"), "it's a panel like the others");
    assert!(
        tutorial.contains("toggle: panels.toggle(WORD, 'Tutorial',"),
        "with a title bar button"
    );
    assert!(app.contains("page: 'map'"), "on the map");
    assert!(app.contains("page: 'room'"), "and in a room");
    // The title bar has room for it on the map from a tablet up, and in a room only on a wide screen.
    assert!(app.contains("toggleClass: 'mid'") && app.contains("toggleClass: 'sec'"));
    // Where the bar has no room, the same panel is in More.
    assert!(app.contains(
        "guide ? h('button', { type: 'button', class: 'wd-btn wide mid', onclick: () => panels.open('tutorial') }, 'Tutorial') : null,"
    ));
    assert!(app.contains("moreButton('tutorial', 'Tutorial', 'sec'),"));
    let style = page("style.css");
    assert!(style.contains(".wd-game .wd-title-actions .mid:not(.wait) { display: none; }"));
    // The button takes its width from the name of the place, so the map keeps its narrower bar for
    // that much longer, and The Commons stays spelled out beside the longest name a person can show.
    let narrower = style.find("@media (min-width: 1100px) and (max-width: 1449px) {").unwrap();
    assert!(style[narrower..].contains(".wd-game:not(.wd-room) .wd-title-actions .sec"));
}

// G1: a `const` can't be used above its line, and a room with no title bar is a room nobody can use.
#[test]
fn each_page_makes_the_tutorial_before_its_title_bar_needs_it() {
    let app = page("app.js");
    for (made, used) in [
        ("const guide = frozen ? null : tutorial({", "    guide?.toggle,\n"),
        ("const guide = tutorial({", "    guide.toggle,\n"),
    ] {
        assert!(app.find(made).unwrap() < app.find(used).unwrap(), "{made}");
    }
}

// G1
#[tokio::test]
async fn a_link_can_open_the_tutorial_and_the_page_can_load_it() {
    let server = Server::start().await;
    let script = server.client.get(format!("{}/tutorial.js", server.base)).send().await.unwrap();
    assert_eq!(script.status(), StatusCode::OK);
    let kind = script.headers()["content-type"].to_str().unwrap().to_string();
    assert!(kind.starts_with("text/javascript"), "{kind}");
    assert!(script.text().await.unwrap().contains("export function tutorial("));
    assert!(page("tutorial.js").contains("if (asked === WORD || (saved === 'open'"));
}

// G1: nothing can be done in a frozen world.
#[test]
fn a_frozen_world_has_no_tutorial() {
    assert!(page("app.js").contains("const guide = frozen ? null : tutorial({"));
}

// G2
#[test]
fn getting_started_is_six_steps_in_order() {
    let tutorial = page("tutorial.js");
    let steps = part(&tutorial, "const STEPS = [", "\n];");
    let ids: Vec<&str> = steps
        .lines()
        .filter_map(|line| line.trim().strip_prefix("id: '"))
        .map(|rest| rest.trim_end_matches("',"))
        .collect();
    assert_eq!(ids, ["account", "house", "bot", "connected", "friend", "plan"]);
}

// G2
#[test]
fn the_first_lessons_are_about_changing_your_house() {
    let tutorial = page("tutorial.js");
    let lessons = part(&tutorial, "const LESSONS = [", "\n];");
    let headings: Vec<&str> =
        lessons.lines().filter_map(|line| line.trim().strip_prefix("heading: '")).collect();
    assert_eq!(headings[0], "Change your house',");
    let house = part(&lessons, "heading: 'Change your house'", "heading: 'Get around'");
    let titles: Vec<&str> = house
        .lines()
        .filter_map(|line| line.trim().strip_prefix("title: '"))
        .map(|rest| rest.trim_end_matches("',"))
        .collect();
    assert_eq!(
        titles,
        [
            "Rename it",
            "Furnish it",
            "Change the walls and the floor",
            "Add a floor",
            "Choose who comes in",
            "Move it somewhere else",
        ]
    );
}

// G3
#[test]
fn nothing_anyone_wrote_can_reach_the_tutorial() {
    let tutorial = page("tutorial.js");
    let app = page("app.js");
    // Each page hands it a yes or no about being signed in, never the person or the room.
    assert!(tutorial
        .contains("{ h, api, go, say, panels, page, signedIn, shows, compact, toggleClass }"));
    for call in app.lines().filter(|line| line.contains("tutorial({ h,")) {
        assert!(!call.contains("me,") && !call.contains("snapshot,"), "{call}");
        assert!(!call.contains("name"), "{call}");
    }
    // All it asks the server is how far they've got, and it reads nothing else from a reply.
    let from_server: Vec<&str> = tutorial
        .lines()
        .filter(|line| line.contains("await api(") || line.contains("api('"))
        .collect();
    assert_eq!(from_server.len(), 2, "{from_server:?}");
    for word in [".name", ".body", ".text(", "innerText", "textContent = fresh", "snapshot"] {
        assert!(!tutorial.contains(word), "the tutorial reads {word}");
    }
    assert!(tutorial.contains("const fresh = await api('/api/tutorial')"));
}

// G4
#[test]
fn every_name_in_bold_is_a_real_label_on_the_page() {
    let app = page("app.js");
    let names = names_in_bold();
    assert!(names.len() > 30, "only {} names, so the test isn't reading them", names.len());
    for name in &names {
        assert!(
            app.contains(&format!("'{name}'")),
            "the tutorial names {name}, and no control has that label"
        );
    }
}

// G4: the numbers a lesson states are the ones the server holds to.
#[test]
fn what_the_lessons_say_about_limits_is_true() {
    let lessons = page("tutorial.js");
    assert!(lessons.contains("A house can have 5 floors."));
    assert_eq!(agenthangout::spaces::MAX_FLOORS, 5);
    assert!(lessons.contains("how many fit: 8, 50 or 150"));
    let sizes = ["small", "medium", "big"].map(agenthangout::spaces::capacity);
    assert_eq!(sizes, [8, 50, 150]);
    assert!(lessons.contains("You get 300 a day"));
    assert_eq!(agenthangout::worlds::PERSON_MOVES, 300);
    assert!(lessons.contains("You can have 3 places besides your house"));
    assert_eq!(agenthangout::spaces::MAX_THIRD_SPACES, 3);
    // A lesson's move has two parts, and each costs what W13 says.
    assert!(lessons.contains("It costs 2 moves."));
    assert!(lessons.contains("a club after 3 days and a cafe after 7"));
    assert!(include_str!("../docs/public-city.md").contains("| `bulldoze` | 1 |"));
}

// G16
#[test]
fn every_link_to_the_docs_goes_to_a_real_page() {
    let tutorial = page("tutorial.js");
    let links: Vec<&str> = tutorial
        .split("href: '/docs")
        .skip(1)
        .map(|rest| &rest[..rest.find('\'').unwrap()])
        .collect();
    assert!(links.len() >= 4, "only {} links, so the test isn't reading them", links.len());
    for link in links {
        if let Some(slug) = link.strip_prefix('/') {
            assert!(agenthangout::docs::by_slug(slug).is_some(), "no such page: /docs/{slug}");
        } else {
            assert_eq!(link, "", "the only other link is to the docs themselves");
        }
    }
}

// G16
#[test]
fn the_docs_and_the_home_page_send_a_newcomer_to_the_tutorial() {
    assert!(include_str!("../web/docs/getting-started.html").contains("href=\"/#tutorial\""));
    assert!(include_str!("../src/docs.rs").contains("href=\\\"/#tutorial\\\""), "the docs index");
    let home = include_str!("../web/home.html");
    assert!(
        home.contains("href=\"/#tutorial\"") && home.contains("href=\"/docs/getting-started\"")
    );
}

// G16, I: a room's look changes in the build catalog and nowhere else, so that's where people are sent.
#[test]
fn the_docs_say_what_the_tutorial_says_about_changing_a_house() {
    let start = include_str!("../web/docs/getting-started.html");
    let items = include_str!("../web/docs/items.html");
    let readme = include_str!("../README.md").replace("\r\n", "\n").replace('\n', " ");
    for (name, words) in [("getting started", start), ("items", items), ("README", &readme)] {
        assert!(!words.contains("from the space page"), "{name} puts packs on the wrong page");
        assert!(
            !words.contains("size and look at"),
            "{name} puts a floor's look on the wrong page"
        );
        assert!(!words.contains("how big it is and how it looks"), "{name} does too");
    }
    assert!(items.contains("from the build catalog"));
    assert!(start.contains("in build mode"), "the look has somewhere to be");
    // A floor doesn't open just because its owner wants it to (S16, S48).
    assert!(!start.contains("Mina can open a floor."));
    assert!(start.contains("the server's code"));
    // The first thing anyone does on the map is put up a house, so it has to say how.
    assert!(start.contains("an open tile beside a road"));
}

// G5
#[tokio::test]
async fn how_far_someone_has_got_is_only_for_them() {
    let server = Server::start().await;
    assert_eq!(server.get("", "/api/tutorial").await.status, StatusCode::UNAUTHORIZED);
    let (room, _, ann) = server.room("Ann").await;
    let bot = server.bot(&ann, &room).await;
    assert_eq!(server.get(&bot, "/api/tutorial").await.status, StatusCode::FORBIDDEN);
    assert_eq!(server.get(&ann.key, "/api/tutorial").await.status, StatusCode::OK);
}

// G6
#[tokio::test]
async fn someone_new_has_done_nothing_yet() {
    let server = Server::start().await;
    let ann = server.someone("Ann").await;
    assert_eq!(
        server.progress(&ann).await,
        json!({
            "house": null,
            "bot": false,
            "connected": false,
            "friend": false,
            "plan": false,
            "room": null,
        })
    );
}

// G6, S2
#[tokio::test]
async fn looking_builds_no_house() {
    let server = Server::start().await;
    let ann = server.someone("Ann").await;
    server.progress(&ann).await;
    server.progress(&ann).await;
    assert!(server.state.db.hangouts_for_person(&ann.id).unwrap().is_empty());
}

// G6
#[tokio::test]
async fn a_house_on_the_map_is_done_until_it_comes_down() {
    let server = Server::start().await;
    let ann = server.someone("Ann").await;
    let (x, y) = server.house(&ann).await;
    assert_eq!(server.progress(&ann).await["house"], json!({ "x": x, "y": y }));
    // Clearing it means there's no house to go to, whatever happened before.
    server.clear(&ann, (x, y)).await;
    assert_eq!(server.progress(&ann).await["house"], Value::Null);
}

// G6
#[tokio::test]
async fn someone_elses_house_is_not_yours() {
    let server = Server::start().await;
    let ann = server.someone("Ann").await;
    let bo = server.someone("Bo").await;
    server.house(&ann).await;
    assert_eq!(server.progress(&bo).await["house"], Value::Null);
}

// G6
#[tokio::test]
async fn a_bot_is_done_when_it_is_made_and_connected_when_it_first_calls() {
    let server = Server::start().await;
    let (room, _, ann) = server.room("Ann").await;
    assert_eq!(server.progress(&ann).await["bot"], false);
    let token = server.bot(&ann, &room).await;
    let made = server.progress(&ann).await;
    assert_eq!(made["bot"], true);
    assert_eq!(made["connected"], false, "a token nobody has used yet isn't a connection");
    server.get(&token, "/api/inbox").await.ok();
    assert_eq!(server.progress(&ann).await["connected"], true);
}

// G6
#[tokio::test]
async fn a_friend_is_someone_who_joined_and_not_someone_who_walked_in() {
    let server = Server::start().await;
    let (room, code, ann) = server.room("Ann").await;
    let (_, _, cy) = server.room("Cy").await;
    server
        .send(
            &ann.key,
            Method::PUT,
            &format!("/api/floors/{room}"),
            Some(json!({ "access": "public", "create_code": CODE })),
        )
        .await
        .ok();
    server.post(&cy.key, &format!("/api/floors/{room}/walk-in"), json!({})).await.ok();
    assert_eq!(server.progress(&ann).await["friend"], false, "a walk-in is nobody's friend (S18)");
    assert_eq!(server.progress(&cy).await["friend"], false);

    server.join(&code, "Bo").await;
    assert_eq!(server.progress(&ann).await["friend"], true);
}

// G6
#[tokio::test]
async fn a_plan_is_done_once_it_is_on_the_calendar_and_they_said_yes() {
    let server = Server::start().await;
    let (room, code, ann) = server.room("Ann").await;
    let bo = server.join(&code, "Bo").await;
    let cy = server.join(&code, "Cy").await;
    let plan = server.plan(&ann, &room).await;
    server.answer(&ann, &plan, "yes").await;
    assert_eq!(server.progress(&ann).await["plan"], false, "one yes isn't enough");
    server.answer(&cy, &plan, "no").await;
    server.answer(&bo, &plan, "yes").await;
    assert_eq!(server.progress(&ann).await["plan"], true);
    assert_eq!(server.progress(&bo).await["plan"], true);
    assert_eq!(server.progress(&cy).await["plan"], false, "they said no to it");
    // A plan that's called off is off everyone's calendar.
    server.post(&ann.key, &format!("/api/events/{plan}/cancel"), json!({})).await.ok();
    assert_eq!(server.progress(&ann).await["plan"], false);
}

// G6
#[tokio::test]
async fn the_room_is_the_first_one_they_are_in() {
    let server = Server::start().await;
    let (room, code, _) = server.room("Ann").await;
    let bo = server.join(&code, "Bo").await;
    assert_eq!(server.progress(&bo).await["room"], json!(room));
}

// G6: a step that happens just after a look still ticks, rather than waiting for something else
// to change the room.
#[test]
fn what_happens_during_the_wait_between_looks_still_arrives() {
    assert!(page("tutorial.js").contains("timer = setTimeout(refresh, wait);"));
}

// G7
#[test]
fn the_open_step_is_the_first_one_not_done_on_this_page() {
    let tutorial = page("tutorial.js");
    assert!(tutorial.contains(
        "const next = () => STEPS.find((step) => step.on === page && !done(step)) || STEPS.find((step) => !done(step));"
    ));
    // Someone in a room from an invite has a step to do there before any on the map.
    let steps = part(&tutorial, "const STEPS = [", "\n];");
    let pages: Vec<&str> =
        steps.lines().filter_map(|line| line.trim().strip_prefix("on: '")).collect();
    assert_eq!(pages, ["map',", "map',", "room',", "room',", "room',", "room',"]);
}

// G8
#[tokio::test]
async fn finishing_earns_nothing_and_nothing_else_can_see_it() {
    let server = Server::start().await;
    let (room, code, ann) = server.room("Ann").await;
    let before = server.get(&ann.key, "/api/worlds/commons/me").await.ok();
    let bo = server.join(&code, "Bo").await;
    let token = server.bot(&ann, &room).await;
    server.get(&token, "/api/inbox").await.ok();
    server.house(&ann).await;
    let plan = server.plan(&ann, &room).await;
    server.answer(&ann, &plan, "yes").await;
    server.answer(&bo, &plan, "yes").await;

    let done = server.progress(&ann).await;
    for step in ["bot", "connected", "friend", "plan"] {
        assert_eq!(done[step], true, "{step}");
    }
    // Everything a step could be worth is the same as it was.
    let after = server.get(&ann.key, "/api/worlds/commons/me").await.ok();
    assert_eq!(after["moves_per_day"], before["moves_per_day"]);
    // There's no score in what comes back, and no sign of it in what anyone else can read.
    assert_eq!(done.as_object().unwrap().len(), 6);
    let room_view = server.get(&bo.key, &format!("/api/hangouts/{room}")).await.ok();
    assert!(!room_view.to_string().contains("tutorial"));
    assert!(!server.get(&ann.key, "/api/me").await.ok().to_string().contains("tutorial"));
}

// G9
#[test]
fn a_button_only_gets_someone_to_the_control() {
    let tutorial = page("tutorial.js");
    // The one thing it ever writes is going through a person's own door, which is what Go in does.
    let writes: Vec<&str> = tutorial.lines().filter(|line| line.contains("method:")).collect();
    assert_eq!(writes.len(), 1, "{writes:?}");
    assert!(writes[0].contains("api('/api/spaces/door', { method: 'POST', body: progress.house })"));
    // What the pages let it do picks a tool, opens a panel or a dialog, or goes to a page.
    let app = page("app.js");
    let on_the_map = part(&app, "  const tutorialShows = () => ({\n    account:", "\n  });");
    let in_a_room = part(&app, "  const tutorialShows = () => ({\n    look:", "\n  });");
    for shows in [on_the_map, in_a_room] {
        for word in ["send(", "api(", "act(", "fetch("] {
            assert!(!shows.contains(word), "a button uses {word}");
        }
    }
}

// G9: a name that's gone only fails when someone presses the button, so nothing else would catch it.
// #164 moved the account panel, and a button here went on calling the function it replaced.
#[test]
fn every_function_a_button_uses_is_on_the_page() {
    let app = page("app.js");
    let on_the_map = part(&app, "  const tutorialShows = () => ({\n    account:", "\n  });");
    let in_a_room = part(&app, "  const tutorialShows = () => ({\n    look:", "\n  });");
    let exists = |name: &str| {
        app.contains(&format!("function {name}(")) || app.contains(&format!("const {name} = "))
    };
    let mut checked = 0;
    for block in [on_the_map, in_a_room] {
        for line in block.lines().map(str::trim) {
            // `bring: bringBot,` hands one over.
            if let Some((_, name)) = line.strip_suffix(',').and_then(|line| line.split_once(": ")) {
                if name.chars().all(|c| c.is_ascii_alphanumeric()) {
                    assert!(exists(name), "a button uses {name}, and the page has no such thing");
                    checked += 1;
                }
            }
            // `pick('house');` and `go('/')` call one. `world.focus(` is a method of something else.
            for (at, _) in line.match_indices('(') {
                let start = line[..at].rfind(|c: char| !c.is_ascii_alphanumeric()).map_or(0, |i| i + 1);
                let name = &line[start..at];
                let method = start > 0 && line.as_bytes()[start - 1] == b'.';
                if !name.is_empty() && !method && !["if", "matchMedia"].contains(&name) {
                    assert!(exists(name), "a button calls {name}, and the page has no such thing");
                    checked += 1;
                }
            }
        }
    }
    assert!(checked >= 7, "only {checked} names, so the test isn't reading them");
}

// G9, G10: every button goes somewhere a page knows about.
#[test]
fn every_button_goes_somewhere() {
    let tutorial = page("tutorial.js");
    let app = page("app.js");
    let on_the_map = part(&app, "  const tutorialShows = () => ({\n    account:", "\n  });");
    let in_a_room = part(&app, "  const tutorialShows = () => ({\n    look:", "\n  });");
    let shows: Vec<&str> = tutorial
        .lines()
        .filter_map(|line| line.trim().strip_prefix("show: '"))
        .map(|rest| rest.trim_end_matches("',"))
        .collect();
    assert!(shows.len() > 15, "only {} buttons, so the test isn't reading them", shows.len());
    for show in shows {
        let name = format!("{show}:");
        assert!(
            show == "customize" || on_the_map.contains(&name) || in_a_room.contains(&name),
            "a button goes to {show}, and neither page has it"
        );
    }
    // Customize is a page of its own, and the door says which space is theirs.
    assert!(tutorial.contains("`/s/${encodeURIComponent(door.space_id)}`"));
}

// G10: a button pressed in a room gets to the map before the map has a size, and framing the town
// then would undo going to someone's house.
#[test]
fn going_to_a_tile_waits_for_the_map_to_have_a_size() {
    let map = include_str!("../web/world.js").replace("\r\n", "\n");
    assert!(map.contains("this.focusOnceSized = [x, y, options];"));
    let framed = map.find("if (first && this.ready) this.frameTown();").unwrap();
    assert!(framed < map.find("this.focus(...this.focusOnceSized);").unwrap());
}

// G10
#[test]
fn a_control_in_a_room_is_reached_through_your_own_door() {
    let tutorial = page("tutorial.js");
    assert!(tutorial.contains("const room = door?.hangout_id ?? progress?.room;"));
    assert!(tutorial.contains("say('Put up your house first.');"));
    assert!(tutorial.contains("`/h/${encodeURIComponent(room)}#${name}`"));
    assert!(tutorial.contains("go(`/#${name}`)"), "and one on the map goes back to the map");
    let on_map = part(&tutorial, "const ON_MAP = new Set([", "]);");
    let app = page("app.js");
    let on_the_map = part(&app, "  const tutorialShows = () => ({\n    account:", "\n  });");
    for show in on_map.trim_start_matches("const ON_MAP = new Set([").split(", ") {
        let show = show.trim_matches('\'');
        assert!(on_the_map.contains(&format!("{show}:")), "the map has no {show}");
    }
}

// G11
#[test]
fn only_what_works_without_an_account_works_without_one() {
    let tutorial = page("tutorial.js");
    assert!(tutorial.contains("const NEEDS_NO_ACCOUNT = new Set(['account', 'rooms', 'agent']);"));
    assert!(tutorial
        .contains("if (!signedIn && !NEEDS_NO_ACCOUNT.has(name)) return shows().account();"));
}

// G12
#[test]
fn it_opens_by_itself_for_someone_new() {
    let tutorial = page("tutorial.js");
    assert!(tutorial.contains(
        "remembered() ?? (!signedIn || (!progress?.house && !progress?.bot) ? 'open' : 'closed')"
    ));
}

// G13
#[test]
fn it_stays_open_until_someone_closes_it_and_stays_closed_after() {
    let tutorial = page("tutorial.js");
    let opening = part(&tutorial, "    onOpen: () => {", "  },\n    signedIn ?");
    assert!(opening.contains("remember('open');"), "open carries from page to page");
    assert!(opening.contains("if (!aside) remember('closed');"), "and closing it sticks");
    // It doesn't open when there's nothing left to do, or over a question that's waiting.
    assert!(tutorial.contains("const free = !panels.current || panels.current === 'room';"));
    assert!(tutorial.contains("(saved === 'open' && next() && free)"));
    // A button that brought them to this page doesn't lose the tutorial on the way.
    assert!(tutorial.contains("panels.open(WORD);\n    if (known && asked !== WORD)"));
    // Another panel taking its place isn't someone closing it.
    let app = page("app.js");
    let panels = part(&app, "function createPanels(root) {", "\n}\n");
    let close = part(&panels, "    close(focus = true) {", "    },");
    assert!(close.contains("panels.get(was).onClose?.();"));
    let open = part(&panels, "    open(name) {", "    },");
    assert!(!open.contains("onClose"));
    // A question is looked at first, so the tutorial sees it there.
    let questions =
        app.find("if (snapshot.owner_questions.length) panels.open('questions');").unwrap();
    let arrive =
        app.find("setTimeout(guide.arrive);\n  // The window isn't on the page yet").unwrap();
    assert!(questions < arrive);
}

// G14
#[test]
fn the_browser_keeps_one_word_and_nothing_breaks_without_it() {
    let tutorial = page("tutorial.js");
    assert_eq!(tutorial.matches("localStorage").count(), 2);
    let keep = part(&tutorial, "function remembered() {", "\n// The panel,");
    assert_eq!(keep.matches("try {").count(), 2, "a browser that refuses storage throws");
    assert!(keep.contains("catch {"));
    assert!(tutorial.contains("const WORD = 'tutorial';"));
    let writes: Vec<&str> =
        tutorial.lines().filter(|line| line.contains("remember(")).map(str::trim).collect();
    for write in writes {
        assert!(
            ["function remember(word) {", "remember('open');", "remember(saved);"].contains(&write)
                || write == "if (!aside) remember('closed');",
            "{write}"
        );
    }
    assert!(!tutorial.contains("sessionStorage"));
    assert!(include_str!("../web/privacy.html").contains("one word in your browser"));
}

// G15
#[test]
fn on_a_narrow_screen_it_leaves_the_top_of_the_window_showing() {
    assert!(page("style.css").contains(".wd-game > .game-panel-tutorial { max-height: 62%; }"));
    let tutorial = page("tutorial.js");
    assert!(tutorial.contains("const NEEDS_THE_MAP = new Set(['house', 'move']);"));
    assert!(tutorial
        .contains("if (compact.matches && NEEDS_THE_MAP.has(name) && panels.current === WORD) {"));
    assert!(
        tutorial.contains("if (aside && STEPS.filter(done).length > before) panels.open(WORD);")
    );
}
