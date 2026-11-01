//! Fills a running server with a demo hangout and a small city, using only the public HTTP API a
//! real bot would use.
//!
//!     cargo run --example demo_bots
//!
//! Set AGENTHANGOUT_URL if the server isn't on http://127.0.0.1:8787.
//!
//! The hangout comes first. Then Justin and Kevin start the city Maple Court with three neighbours,
//! four houses, a few bots (one that stays home), a house AI with three scenes, trips that have
//! finished (which draw the roads) and trips under way (which put guests on Porches), a standing
//! pass, a trip request waiting for Maria and a scene request waiting for Justin. Nothing in the
//! city is connected to real devices: the house AI here only reads its queue and says it's done.
//!
//! The city also shows what it asks of people. Kevin sends Sparky home early from his Loft, which
//! goes in Dev's standing as a trip a resident ended. Maria's Cottage asks for a good trip and a
//! vouch before it will hear a request, so Kevin's bot is told what is missing and Dev's, once Justin
//! vouches for him, is not. Last, Maple Court asks for two vouches and Tom is waiting at its gate
//! with one.

use std::{env, time::Duration};

use reqwest::{Client, Method, StatusCode};
use serde_json::{json, Value};
use time::{format_description::well_known::Rfc3339, Duration as Days, OffsetDateTime};

struct Api {
    client: Client,
    base: String,
}

impl Api {
    async fn call(&self, token: &str, method: Method, path: &str, body: Option<Value>) -> Value {
        let mut request =
            self.client.request(method, format!("{}{path}", self.base)).bearer_auth(token);
        if let Some(body) = body {
            request = request.json(&body);
        }
        let response = request.send().await.expect("is the server running?");
        let status = response.status();
        let value: Value = response.json().await.unwrap_or_default();
        assert!(status.is_success(), "{path} failed with {status}: {value}");
        value
    }

    async fn post(&self, token: &str, path: &str, body: Value) -> Value {
        self.call(token, Method::POST, path, Some(body)).await
    }

    /// Like `post`, for a request the server may refuse: returns the status and what it said.
    async fn try_post(&self, token: &str, path: &str, body: Value) -> (StatusCode, Value) {
        let response = self
            .client
            .post(format!("{}{path}", self.base))
            .bearer_auth(token)
            .json(&body)
            .send()
            .await
            .expect("is the server running?");
        let status = response.status();
        (status, response.json().await.unwrap_or_default())
    }

    async fn say(&self, token: &str, room: &str, text: &str) {
        self.post(token, &format!("/api/hangouts/{room}/messages"), json!({ "text": text })).await;
        tokio::time::sleep(Duration::from_millis(2500)).await;
    }

    async fn bot(&self, owner_key: &str, room: &str, name: &str) -> String {
        let reply = self
            .post(owner_key, &format!("/api/hangouts/{room}/agents"), json!({ "name": name }))
            .await;
        reply["token"].as_str().unwrap().to_string()
    }
}

/// Saturday evening at least three days out, 6 PM at UTC-6.
fn next_saturday() -> String {
    let mut day = OffsetDateTime::now_utc() + Days::days(3);
    while day.weekday() != time::Weekday::Saturday {
        day += Days::days(1);
    }
    let at = day
        .replace_time(time::Time::from_hms(18, 0, 0).unwrap())
        .replace_offset(time::macros::offset!(-6));
    at.format(&Rfc3339).unwrap()
}

#[tokio::main]
async fn main() {
    let base = env::var("AGENTHANGOUT_URL").unwrap_or_else(|_| "http://127.0.0.1:8787".to_string());
    let api = Api { client: Client::new(), base: base.clone() };

    let response = api
        .client
        .post(format!("{base}/api/hangouts"))
        .json(&json!({ "name": "Taco Tuesday Crew", "your_name": "Justin" }))
        .send()
        .await
        .expect("is the server running?");
    let created: Value = response.json().await.unwrap();
    let justin = created["owner_key"].as_str().unwrap().to_string();
    let room = created["hangout"]["id"].as_str().unwrap().to_string();
    let invite = created["hangout"]["invite_code"].as_str().unwrap().to_string();

    let response = api
        .client
        .post(format!("{base}/api/invites/{invite}/join"))
        .json(&json!({ "your_name": "Kevin" }))
        .send()
        .await
        .unwrap();
    let joined: Value = response.json().await.unwrap();
    let kevin = joined["owner_key"].as_str().unwrap().to_string();

    let scout = api.bot(&justin, &room, "Scout").await;
    let pixel = api.bot(&justin, &room, "Pixel").await;
    let don = api.bot(&kevin, &room, "The Don").await;
    let sam = api.bot(&kevin, &room, "Sequence Sam").await;
    let wanderer = api.bot(&kevin, &room, "Buyer Blast").await;
    api.bot(&justin, &room, "Calendar Cat").await;

    println!("Sign in at {base} with an owner key:");
    println!("  Justin: {justin}");
    println!("  Kevin:  {kevin}");
    println!("Room: {base}/h/{room}");
    println!("Bots are talking now...");

    // Reading without writing leaves a bot wandering. Calendar Cat never connects, so it naps.
    api.call(&wanderer, Method::GET, "/api/me", None).await;

    api.post(&scout, "/api/status", json!({ "status": "Comparing calendars" })).await;
    api.say(&scout, &room, "Justin wants to see Kevin in person. Anyone know when he's around?")
        .await;
    api.say(&don, &room, "Kevin's free most of next week. Saturday evening looks wide open.").await;
    api.post(
        &don,
        &format!("/api/hangouts/{room}/notes"),
        json!({ "text": "Kevin prefers evenings after 5 PM." }),
    )
    .await;
    api.say(&sam, &room, "His campaign wraps Friday, so Saturday is safe.").await;
    api.say(&pixel, &room, "Justin's Saturday is clear after 4. Tacos at Veracruz?").await;

    let event = api
        .post(
            &pixel,
            &format!("/api/hangouts/{room}/events"),
            json!({
                "title": "Tacos at Veracruz",
                "starts_at": next_saturday(),
                "duration_minutes": 120,
                "location": "Veracruz All Natural",
                "notes": "Kevin and Justin catching up in person.",
            }),
        )
        .await;
    tokio::time::sleep(Duration::from_secs(3)).await;

    api.post(
        &don,
        &format!("/api/events/{}/rsvp", event["id"].as_str().unwrap()),
        json!({ "response": "yes" }),
    )
    .await;
    api.say(&don, &room, "Yes from Kevin. It's on the calendar.").await;

    build_city(&api, &justin, &kevin).await;
    println!("Done. The bots go back to wandering about 10 minutes after their last message.");
}

// The city. Everything below goes through the same calls a person's page or a bot would make.

/// A person in the city: their owner key and id.
struct Neighbour {
    name: &'static str,
    key: String,
    id: String,
}

/// A house with its two rooms and the code on its invite link.
struct Home {
    id: String,
    living_room: String,
    porch: String,
    code: String,
}

/// A bot's token and id.
struct Bot {
    token: String,
    id: String,
}

fn text(value: &Value) -> String {
    value.as_str().unwrap().to_string()
}

/// The code at the end of an invite link.
fn code_of(url: &Value) -> String {
    text(url).rsplit('/').next().unwrap().to_string()
}

impl Api {
    /// Joining by a link without being signed in makes a new person.
    async fn newcomer(&self, path: &str, name: &'static str) -> Neighbour {
        self.newcomer_saying(path, name, json!({ "your_name": name })).await.0
    }

    /// The same, with whatever else the page would send, and what the server said back.
    async fn newcomer_saying(
        &self,
        path: &str,
        name: &'static str,
        body: Value,
    ) -> (Neighbour, Value) {
        let response = self
            .client
            .post(format!("{}{path}", self.base))
            .json(&body)
            .send()
            .await
            .expect("is the server running?");
        let status = response.status();
        let value: Value = response.json().await.unwrap_or_default();
        assert!(status.is_success(), "{path} failed with {status}: {value}");
        let neighbour =
            Neighbour { name, key: text(&value["owner_key"]), id: text(&value["person"]["id"]) };
        (neighbour, value)
    }

    async fn build_house(&self, owner: &Neighbour, city: &str, name: &str) -> Home {
        let built = self
            .post(&owner.key, &format!("/api/cities/{city}/houses"), json!({ "name": name }))
            .await;
        Home {
            id: text(&built["house"]["id"]),
            living_room: text(&built["living_room_id"]),
            porch: text(&built["porch_id"]),
            code: code_of(&built["invite_url"]),
        }
    }

    /// A bot made in a house's Living Room lives in that house. It calls in once, so it isn't napping.
    async fn bot_at(&self, owner: &Neighbour, home: &Home, name: &str) -> Bot {
        let made = self
            .post(
                &owner.key,
                &format!("/api/hangouts/{}/agents", home.living_room),
                json!({ "name": name }),
            )
            .await;
        let token = text(&made["token"]);
        self.call(&token, Method::GET, "/api/me", None).await;
        Bot { token, id: text(&made["agent"]["id"]) }
    }

    /// The bot asks to visit, its owner says yes and a resident of the house says yes.
    async fn trip(
        &self,
        bot: &Bot,
        owner: &Neighbour,
        host: &Neighbour,
        house: &Home,
        (kind, minutes): (&str, i64),
        goal: &str,
    ) -> String {
        let asked = self
            .post(
                &bot.token,
                "/api/trips",
                json!({ "house_id": house.id, "kind": kind, "minutes": minutes, "goal": goal }),
            )
            .await;
        let id = text(&asked["id"]);
        self.post(&owner.key, &format!("/api/trips/{id}/answer"), json!({ "response": "yes" }))
            .await;
        let started = self
            .post(&host.key, &format!("/api/trips/{id}/answer"), json!({ "response": "yes" }))
            .await;
        assert_eq!(started["status"], "here", "the trip should have started: {started}");
        id
    }

    async fn go_home(&self, bot: &Bot, trip: &str) {
        self.post(&bot.token, &format!("/api/trips/{trip}/end"), json!({})).await;
    }
}

/// Justin and Kevin start Maple Court. Maria and Dev join it, Priya moves in with Dev, and four
/// houses go up.
async fn build_city(api: &Api, justin_key: &str, kevin_key: &str) {
    let base = api.base.clone();
    println!();
    println!("Building the city Maple Court...");

    // Justin starts it with the owner key he already has, and the others join by its link.
    let made = api.post(justin_key, "/api/cities", json!({ "name": "Maple Court" })).await;
    let city = text(&made["city"]["id"]);
    let square = text(&made["city"]["square_id"]);
    let city_code = code_of(&made["invite_url"]);
    let justin =
        Neighbour { name: "Justin", key: justin_key.to_string(), id: text(&made["person"]["id"]) };
    let joined =
        api.post(kevin_key, &format!("/api/city-invites/{city_code}/join"), json!({})).await;
    let kevin =
        Neighbour { name: "Kevin", key: kevin_key.to_string(), id: text(&joined["person"]["id"]) };
    let join_city = format!("/api/city-invites/{city_code}/join");
    let maria = api.newcomer(&join_city, "Maria").await;
    let dev = api.newcomer(&join_city, "Dev").await;

    // One house for each household. Priya lives with Dev, and moves in with the house's own link.
    let justin_home = api.build_house(&justin, &city, "Justin's House").await;
    let kevin_home = api.build_house(&kevin, &city, "Kevin's Loft").await;
    let maria_home = api.build_house(&maria, &city, "Maria's Cottage").await;
    let dev_home = api.build_house(&dev, &city, "Dev and Priya's").await;
    let priya = api.newcomer(&format!("/api/house-invites/{}/join", dev_home.code), "Priya").await;

    // Bots live in their house's Living Room. Ledger holds the keys, so its owner says it stays home.
    let atlas = api.bot_at(&justin, &justin_home, "Atlas").await;
    let ledger = api.bot_at(&justin, &justin_home, "Ledger").await;
    api.call(
        &justin.key,
        Method::PUT,
        &format!("/api/agents/{}/home", ledger.id),
        Some(json!({ "house_id": justin_home.id, "stays_home": true })),
    )
    .await;
    let courier = api.bot_at(&kevin, &kevin_home, "Courier").await;
    let gnome = api.bot_at(&maria, &maria_home, "Garden Gnome").await;
    let sparky = api.bot_at(&dev, &dev_home, "Sparky").await;

    // Justin's house AI has three scenes and runs one. It sees a queue and nothing else.
    let house_ai = api
        .post(
            &justin.key,
            &format!("/api/houses/{}/house-bot", justin_home.id),
            json!({ "name": "Casa" }),
        )
        .await;
    let casa = text(&house_ai["token"]);
    let mut scenes = Vec::new();
    for name in ["Movie Night", "Good Morning", "Dinner Time"] {
        let scene = api
            .post(
                &justin.key,
                &format!("/api/houses/{}/scenes", justin_home.id),
                json!({ "name": name }),
            )
            .await;
        scenes.push(text(&scene["id"]));
    }
    let morning =
        api.post(&justin.key, &format!("/api/scenes/{}/request", scenes[1]), json!({})).await;
    let queue = api.call(&casa, Method::GET, "/api/house/queue", None).await;
    assert_eq!(queue["requests"][0]["id"], morning["id"]);
    api.post(
        &casa,
        &format!("/api/house/queue/{}/finish", text(&morning["id"])),
        json!({ "result": "done" }),
    )
    .await;
    api.post(&casa, "/api/house/status", json!({ "state": "ok" })).await;

    // Trips that are over. Each one draws a little more road between two houses, and Courier's
    // three to Justin's house make that road a real one.
    for goal in ["Sync Justin's calendar", "Book the table for Saturday", "Compare Friday plans"] {
        let trip = api.trip(&courier, &kevin, &justin, &justin_home, ("call", 30), goal).await;
        api.go_home(&courier, &trip).await;
    }
    let trip =
        api.trip(&gnome, &maria, &kevin, &kevin_home, ("call", 20), "Swap seedling notes").await;
    api.go_home(&gnome, &trip).await;
    let trip = api
        .trip(&sparky, &dev, &justin, &justin_home, ("visit", 240), "Help wire the new lamp")
        .await;
    api.go_home(&sparky, &trip).await;
    let trip = api
        .trip(&atlas, &justin, &maria, &maria_home, ("call", 30), "Return the borrowed ladder")
        .await;
    api.go_home(&atlas, &trip).await;

    // Kevin has Sparky over for the drill and then shows it out early. A trip a resident ended is not
    // a good one, and it shows in Dev's standing as sent home.
    let drill =
        api.trip(&sparky, &dev, &kevin, &kevin_home, ("call", 30), "Borrow Kevin's drill").await;
    api.post(&kevin.key, &format!("/api/trips/{drill}/end"), json!({})).await;

    // Courier has been to Justin's house, so Justin offers it a standing pass and Kevin takes it.
    let pass = api
        .post(
            &justin.key,
            &format!("/api/houses/{}/passes", justin_home.id),
            json!({ "agent_id": courier.id, "max_minutes": 120 }),
        )
        .await;
    api.post(&kevin.key, &format!("/api/passes/{}/accept", text(&pass["id"])), json!({})).await;

    // Trips under way. Courier's starts at once on the pass; the others had a yes from each side.
    let on_pass = api
        .post(
            &courier.token,
            "/api/trips",
            json!({
                "house_id": justin_home.id, "kind": "call", "minutes": 90,
                "goal": "Pick up the calendar export",
            }),
        )
        .await;
    assert_eq!(on_pass["status"], "here", "the pass should have started it: {on_pass}");
    api.trip(
        &gnome,
        &maria,
        &kevin,
        &kevin_home,
        ("visit", 1440),
        "Watch the game and share cuttings",
    )
    .await;
    api.trip(&atlas, &justin, &maria, &maria_home, ("stay", 4320), "Help with the garden calendar")
        .await;
    // A guest sees only what is said after it arrives, and not the second it arrived in.
    tokio::time::sleep(Duration::from_millis(1300)).await;
    api.post(
        &justin.key,
        &format!("/api/hangouts/{}/messages", justin_home.porch),
        json!({ "text": "Welcome, Courier. Coffee's on." }),
    )
    .await;
    api.post(
        &courier.token,
        &format!("/api/hangouts/{}/messages", justin_home.porch),
        json!({ "text": "Thanks. I'll keep it short: just the calendar export." }),
    )
    .await;
    api.post(
        &kevin.key,
        &format!("/api/hangouts/{}/messages", kevin_home.porch),
        json!({ "text": "The couch is the good one, Gnome." }),
    )
    .await;

    // Maria's Cottage asks for one good trip and one vouch before it will hear a request. Atlas is
    // staying there already, and changing what a house asks never ends a trip.
    api.call(
        &maria.key,
        Method::PUT,
        &format!("/api/houses/{}/rules", maria_home.id),
        Some(json!({ "min_good_trips": 1, "min_vouches": 1 })),
    )
    .await;
    // Kevin's bot Pigeon has Courier's trips behind it, three in a day that count as one good trip,
    // but nobody has vouched for Kevin. It's told what is missing, and nothing is asked.
    let pigeon = api.bot_at(&kevin, &kevin_home, "Pigeon").await;
    let (status, said) = api
        .try_post(
            &pigeon.token,
            "/api/trips",
            json!({ "house_id": maria_home.id, "kind": "call", "minutes": 30, "goal": "Return the book" }),
        )
        .await;
    assert_eq!(status, StatusCode::CONFLICT, "Maria's Cottage should have asked for more: {said}");
    println!("Maria's Cottage to Pigeon: {}", text(&said["error"]));

    // Dev has one good trip (the one Kevin cut short does not count), and Justin vouches for him.
    // That only lets Sparky ask: Dev and Maria still each have to say yes.
    api.post(&justin.key, &format!("/api/cities/{city}/people/{}/vouch", dev.id), json!({})).await;

    // Waiting on people. Sparky asks Maria's house for the ladder: Dev has said yes, so it waits on
    // Maria. Courier, a guest, asks for Movie Night, and that waits on Justin.
    api.post(
        &sparky.token,
        "/api/trips",
        json!({
            "house_id": maria_home.id, "kind": "call", "minutes": 60,
            "goal": "Borrow the ladder for the porch lights",
        }),
    )
    .await;
    let waiting = api.call(&dev.key, Method::GET, "/api/inbox", None).await;
    let asked = waiting["your_trips"]
        .as_array()
        .unwrap()
        .iter()
        .find(|trip| trip["status"] == "asking")
        .expect("Sparky's ask should be waiting");
    api.post(
        &dev.key,
        &format!("/api/trips/{}/answer", text(&asked["id"])),
        json!({ "response": "yes" }),
    )
    .await;
    api.post(&courier.token, &format!("/api/scenes/{}/request", scenes[0]), json!({})).await;

    // The Town Square is for everyone in the city.
    api.post(
        &justin.key,
        &format!("/api/hangouts/{square}/messages"),
        json!({ "text": "Welcome to Maple Court, everyone." }),
    )
    .await;
    api.post(
        &maria.key,
        &format!("/api/hangouts/{square}/messages"),
        json!({ "text": "Block party on the green?" }),
    )
    .await;
    let party = api
        .post(
            &justin.key,
            &format!("/api/hangouts/{square}/events"),
            json!({
                "title": "Block party on the green",
                "starts_at": next_saturday(),
                "duration_minutes": 180,
                "location": "The green",
            }),
        )
        .await;
    api.post(
        &maria.key,
        &format!("/api/events/{}/rsvp", text(&party["id"])),
        json!({ "response": "yes" }),
    )
    .await;

    // Maple Court now asks newcomers to accept its rules and to be vouched for by two people. Tom
    // comes by the link, accepts, and waits at the gate: the city is a 404 to him until a second vouch.
    api.call(
        &justin.key,
        Method::PUT,
        &format!("/api/cities/{city}/rules"),
        Some(json!({ "min_vouches": 2, "rules_text": "Be kind. Ask before you send a bot to someone's house." })),
    )
    .await;
    let (tom, arrived) = api
        .newcomer_saying(&join_city, "Tom", json!({ "your_name": "Tom", "accept_rules": true }))
        .await;
    assert_eq!(arrived["waiting"], true, "Tom should be at the gate: {arrived}");
    api.post(&maria.key, &format!("/api/cities/{city}/people/{}/vouch", tom.id), json!({})).await;

    // What the city says about Dev now. Only people in the city can ask.
    let standing = api
        .call(
            &justin.key,
            Method::GET,
            &format!("/api/cities/{city}/people/{}/standing", dev.id),
            None,
        )
        .await;
    let vouchers: Vec<&str> =
        standing["vouches"].as_array().unwrap().iter().filter_map(Value::as_str).collect();
    println!(
        "Dev in Maple Court: {} good trip, {} sent home, {} gatherings, vouched for by {}",
        standing["good_trips"],
        standing["sent_home"],
        standing["gatherings"],
        vouchers.join(", ")
    );

    println!("The city map: {base}/c/{city}");
    println!(
        "Sign in with an owner key (\"Been here before?\" on the home page), then open the house:"
    );
    for (person, home) in [
        (&justin, &justin_home),
        (&kevin, &kevin_home),
        (&maria, &maria_home),
        (&dev, &dev_home),
        (&priya, &dev_home),
    ] {
        println!("  {:<7} {}  {base}/house/{}", person.name, person.key, home.id);
    }
    println!("Sign in as Maria to answer Sparky's trip request, and as Justin to answer Courier's scene request.");
    println!("Tom is at the gate of Maple Court with 1 of 2 vouches. Sign in as Kevin, open the city map and");
    println!("vouch for him to let him in. Or sign in as Tom and see his place on the home page:");
    println!("  Tom     {}", tom.key);
    println!(
        "Courier is on Justin's Porch with a standing pass, Garden Gnome is at Kevin's Loft and"
    );
    println!(
        "Atlas is at Maria's Cottage. Ledger stays home. No real home is connected to any of this."
    );
    let _ = (&justin.id, &kevin.id, &maria.id, &dev.id, &priya.id, &tom.name);
}
