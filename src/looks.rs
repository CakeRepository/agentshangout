//! What people and bots look like (docs/looks.md): a fixed wardrobe, and who can change whose look.
//! Everything in a look comes from the wardrobe, so nothing typed reaches the drawing. Only what
//! someone chose is kept, and the page works out the rest from the name.

use std::collections::BTreeMap;

use axum::{
    extract::{rejection::JsonRejection, Path, State},
    routing::{get, put},
    Json, Router,
};
use serde::Serialize;
use serde_json::{json, Value};

use crate::{
    auth::Caller,
    db::{Agent, Wearer, WorldPlayer},
    error::{ApiError, ApiResult},
    now, ops, worlds, AppState,
};

/// C9.
pub const MAX_CHANGES_PER_MINUTE: u32 = 60;

/// What's chosen for one figure, by part. A part nobody chose isn't in it (C4).
pub type Look = BTreeMap<String, String>;

/// What a change asks for, by part. `None` takes a part back to what the name gives it (C5).
type Change = BTreeMap<String, Option<String>>;

/// One word a part can be.
#[derive(Clone, Copy, Debug, Serialize)]
pub struct Choice {
    pub choice: &'static str,
    pub label: &'static str,
}

/// Something about a figure that can be chosen, and the words it can be.
#[derive(Clone, Copy, Debug, Serialize)]
pub struct Part {
    pub part: &'static str,
    pub label: &'static str,
    pub choices: &'static [Choice],
}

const fn word(choice: &'static str, label: &'static str) -> Choice {
    Choice { choice, label }
}

/// Shirts and shells come in the same colours. The last eight are the colours people are handed.
const FABRICS: &[Choice] = &[
    word("red", "Red"),
    word("orange", "Orange"),
    word("yellow", "Yellow"),
    word("green", "Green"),
    word("teal", "Teal"),
    word("blue", "Blue"),
    word("indigo", "Indigo"),
    word("purple", "Purple"),
    word("pink", "Pink"),
    word("white", "White"),
    word("grey", "Grey"),
    word("black", "Black"),
    word("sky", "Sky"),
    word("tangerine", "Tangerine"),
    word("mint", "Mint"),
    word("coral", "Coral"),
    word("lilac", "Lilac"),
    word("sun", "Sun"),
    word("aqua", "Aqua"),
    word("orchid", "Orchid"),
];

/// A person and a bot wear the same hats.
const HATS: &[Choice] = &[
    word("none", "No hat"),
    word("cap", "Cap"),
    word("beanie", "Beanie"),
    word("top_hat", "Top hat"),
    word("witch", "Witch hat"),
    word("crown", "Crown"),
    word("party", "Party hat"),
];

/// C1: everything a person can choose. Nothing else can be worn, and nothing typed is part of it.
pub const PERSON_PARTS: [Part; 7] = [
    Part {
        part: "skin",
        label: "Skin",
        choices: &[
            word("cream", "Cream"),
            word("peach", "Peach"),
            word("tan", "Tan"),
            word("caramel", "Caramel"),
            word("umber", "Umber"),
            word("espresso", "Espresso"),
        ],
    },
    Part {
        part: "style",
        label: "Hairstyle",
        choices: &[
            word("short", "Short"),
            word("long", "Long"),
            word("curly", "Curly"),
            word("bun", "Bun"),
            word("mohawk", "Mohawk"),
            word("bald", "Bald"),
        ],
    },
    Part {
        part: "hair",
        label: "Hair colour",
        choices: &[
            word("black", "Black"),
            word("brown", "Brown"),
            word("honey", "Honey"),
            word("blond", "Blond"),
            word("chestnut", "Chestnut"),
            word("grey", "Grey"),
            word("red", "Red"),
            word("pink", "Pink"),
            word("teal", "Teal"),
            word("violet", "Violet"),
        ],
    },
    Part {
        part: "face",
        label: "Face",
        choices: &[
            word("none", "Nothing"),
            word("glasses", "Glasses"),
            word("shades", "Shades"),
            word("moustache", "Moustache"),
            word("beard", "Beard"),
        ],
    },
    Part { part: "hat", label: "Hat", choices: HATS },
    Part { part: "shirt", label: "Shirt", choices: FABRICS },
    Part {
        part: "trousers",
        label: "Trousers",
        choices: &[
            word("navy", "Navy"),
            word("charcoal", "Charcoal"),
            word("khaki", "Khaki"),
            word("pine", "Pine"),
            word("denim", "Denim"),
            word("stone", "Stone"),
        ],
    },
];

/// C1, C2: everything a bot's owner can choose for it.
pub const BOT_PARTS: [Part; 5] = [
    Part { part: "shell", label: "Shell", choices: FABRICS },
    Part {
        part: "head",
        label: "Head",
        choices: &[word("antenna", "Antenna"), word("twin", "Two antennas"), word("wide", "Wide")],
    },
    Part {
        part: "eyes",
        label: "Eyes",
        choices: &[
            word("cyan", "Cyan"),
            word("amber", "Amber"),
            word("pink", "Pink"),
            word("green", "Green"),
            word("white", "White"),
        ],
    },
    Part {
        part: "chest",
        label: "Chest",
        choices: &[word("plain", "Lights"), word("heart", "Heart"), word("star", "Star"), word("bolt", "Bolt")],
    },
    Part { part: "hat", label: "Hat", choices: HATS },
];

fn parts_of(wearer: Wearer) -> &'static [Part] {
    match wearer {
        Wearer::Person => &PERSON_PARTS,
        Wearer::Bot => &BOT_PARTS,
    }
}

fn part_of(wearer: Wearer, word: &str) -> Option<&'static Part> {
    parts_of(wearer).iter().find(|part| part.part == word)
}

fn in_wardrobe(wearer: Wearer, part: &str, choice: &str) -> bool {
    part_of(wearer, part).is_some_and(|part| part.choices.iter().any(|each| each.choice == choice))
}

/// What the name gives a figure. This is the sum pages did before the server took it over (C16), so
/// it counts UTF-16 units as they did, and nobody changed when it moved.
fn name_hash(name: &str) -> u32 {
    name.chars().fold(0, |value: u32, ch| {
        let unit = ch.encode_utf16(&mut [0; 2])[0];
        value.wrapping_mul(31).wrapping_add(u32::from(unit))
    })
}

/// C4: a person's colour, or a bot owner's, as a word. An older account's colour may not be one.
fn fabric_of(color: &str) -> Option<&'static str> {
    let at = crate::db::COLORS.iter().position(|each| *each == color)?;
    Some(FABRICS[FABRICS.len() - crate::db::COLORS.len() + at].choice)
}

/// C4, C16: everything a person has on: what they chose, and the rest from their name and colour.
pub fn person_wears(name: &str, color: &str, chosen: &Look) -> Look {
    const SKIN: [&str; 5] = ["peach", "tan", "caramel", "umber", "cream"];
    const HAIR: [&str; 6] = ["black", "brown", "honey", "blond", "chestnut", "grey"];
    const TROUSERS: [&str; 4] = ["navy", "charcoal", "khaki", "pine"];
    let h = name_hash(name) as usize;
    let parts = [
        ("skin", SKIN[(h >> 3) % SKIN.len()]),
        ("hair", HAIR[(h >> 6) % HAIR.len()]),
        ("style", "short"),
        ("face", "none"),
        ("hat", if (h >> 9).is_multiple_of(3) { "cap" } else { "none" }),
        ("trousers", TROUSERS[(h >> 11) % TROUSERS.len()]),
    ];
    dressed(&parts, fabric_of(color).map(|fabric| ("shirt", fabric)), chosen)
}

/// C4, C16: everything a bot has on. Its eyes follow its shell unless someone chose others.
pub fn bot_wears(name: &str, owner_color: &str, chosen: &Look) -> Look {
    const HEADS: [&str; 3] = ["antenna", "twin", "wide"];
    let parts = [("head", HEADS[name_hash(name) as usize % HEADS.len()]), ("chest", "plain"), ("hat", "none")];
    dressed(&parts, fabric_of(owner_color).map(|fabric| ("shell", fabric)), chosen)
}

fn dressed(parts: &[(&str, &str)], colour: Option<(&str, &str)>, chosen: &Look) -> Look {
    let mut worn: Look = parts.iter().chain(&colour).map(|(part, word)| (part.to_string(), word.to_string())).collect();
    worn.extend(chosen.clone());
    worn
}

/// C16: what each of these has on, by id.
pub fn worn(state: &AppState, wearer: Wearer, ids: &[String]) -> ApiResult<BTreeMap<String, Look>> {
    let mut chosen = looks_of(state, wearer, ids)?;
    let figures = state.db.figures(wearer, ids)?;
    Ok(figures
        .into_iter()
        .map(|(id, (name, color))| {
            let chosen = chosen.remove(&id).unwrap_or_default();
            let look = match wearer {
                Wearer::Person => person_wears(&name, &color, &chosen),
                Wearer::Bot => bot_wears(&name, &color, &chosen),
            };
            (id, look)
        })
        .collect())
}

fn worn_by(state: &AppState, wearer: Wearer, id: &str) -> ApiResult<Look> {
    Ok(worn(state, wearer, &[id.to_string()])?.remove(id).unwrap_or_default())
}

/// C14: what someone signed in has on, for the street, and nothing else about them.
pub fn worn_by_person(state: &AppState, person_id: &str) -> ApiResult<Look> {
    worn_by(state, Wearer::Person, person_id)
}

/// C4, C6: what's chosen for each of these, by id, with anyone who chose nothing left out.
pub fn looks_of(state: &AppState, wearer: Wearer, ids: &[String]) -> ApiResult<BTreeMap<String, Look>> {
    let mut looks = state.db.looks(wearer, ids)?;
    for look in looks.values_mut() {
        look.retain(|part, choice| in_wardrobe(wearer, part, choice));
    }
    looks.retain(|_, look| !look.is_empty());
    Ok(looks)
}

/// C11: what a room's page draws with.
#[derive(Debug, Default, Serialize)]
pub struct Looks {
    pub people: BTreeMap<String, Look>,
    pub bots: BTreeMap<String, Look>,
}

/// C11, C16: for the people with this floor open and the room's bots. A bot has nothing to draw.
pub fn in_room(state: &AppState, caller: &Caller, present: &[String], agents: &[Agent]) -> ApiResult<Looks> {
    if caller.is_agent() {
        return Ok(Looks::default());
    }
    let bots: Vec<String> = agents.iter().map(|agent| agent.id.clone()).collect();
    Ok(Looks { people: worn(state, Wearer::Person, present)?, bots: worn(state, Wearer::Bot, &bots)? })
}

/// C13, C16: for the bots on a city's map. A bot has nothing to draw.
pub fn on_map(state: &AppState, caller: &Caller, bots: &[String]) -> ApiResult<BTreeMap<String, Look>> {
    if caller.is_agent() {
        return Ok(BTreeMap::new());
    }
    worn(state, Wearer::Bot, bots)
}

/// C14: what a world's listed players have on. A guest, and a player someone only claimed, have
/// nothing to wear. Nothing here says whose a look is: it goes in a row with a generated name.
pub fn on_players(state: &AppState, players: &[WorldPlayer]) -> ApiResult<BTreeMap<i64, Look>> {
    let person_of = |player: &WorldPlayer| player.person_id.clone().filter(|_| player.is_person);
    let people: Vec<String> = players.iter().filter_map(person_of).collect();
    let bots: Vec<String> = players.iter().filter_map(|player| player.agent_id.clone()).collect();
    let mut people = worn(state, Wearer::Person, &people)?;
    let mut bots = worn(state, Wearer::Bot, &bots)?;
    Ok(players
        .iter()
        .filter_map(|player| {
            let look = match (person_of(player), &player.agent_id) {
                (Some(person), _) => people.remove(&person),
                (None, Some(bot)) => bots.remove(bot),
                (None, None) => None,
            };
            look.map(|look| (player.id, look))
        })
        .collect())
}

/// C1, C2, C5: all of a change is checked before any of it is kept.
fn change_look(
    state: &AppState,
    wearer: Wearer,
    id: &str,
    change: &Change,
) -> ApiResult<Value> {
    for (part, choice) in change {
        let known = match choice {
            Some(choice) => in_wardrobe(wearer, part, choice),
            None => part_of(wearer, part).is_some(),
        };
        if !known {
            return Err(ApiError::bad_request("that isn't in the wardrobe"));
        }
    }
    let change: Vec<(&str, Option<&str>)> =
        change.iter().map(|(part, choice)| (part.as_str(), choice.as_deref())).collect();
    state.db.change_look(wearer, id, &change)?;
    let mut looks = looks_of(state, wearer, &[id.to_string()])?;
    Ok(json!({ "look": looks.remove(id).unwrap_or_default(), "worn": worn_by(state, wearer, id)? }))
}

/// C10: a person is drawn on the floors they belong to and any they walked onto.
fn refresh_person(state: &AppState, person_id: &str) -> ApiResult<()> {
    for room in state.db.hangouts_for_person(person_id)? {
        ops::refresh(state, &room.id);
    }
    for floor in state.db.walked_onto(person_id, now())? {
        ops::refresh(state, &floor);
    }
    state.worlds.mark_stale(worlds::COMMONS_ID);
    Ok(())
}

/// C10: a bot is drawn in its rooms, including where it's visiting, on city maps, and on The Commons
/// if it's out at a place. However often that's asked for, the view is built once a second (W29).
fn refresh_bot(state: &AppState, agent: &Agent) -> ApiResult<()> {
    for room in state.db.hangouts_for_agent(&agent.id, now())? {
        ops::refresh(state, &room.id);
    }
    for city in state.db.cities_for_person(&agent.owner_id)? {
        ops::refresh_city(state, &city.id);
    }
    state.worlds.mark_stale(worlds::COMMONS_ID);
    Ok(())
}

/// C9, for everything one person changes, because each change refreshes other people's pages.
fn slow_down(state: &AppState, caller: &Caller) -> ApiResult<()> {
    if state.allow_writes(&format!("look:{}", caller.person_id), MAX_CHANGES_PER_MINUTE) {
        return Ok(());
    }
    Err(ApiError::too_many_requests(format!("slow down: {MAX_CHANGES_PER_MINUTE} changes a minute")))
}

// Routes

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/api/looks", get(wardrobe))
        .route("/api/me/look", put(change_own))
        .route("/api/agents/{id}/look", put(change_bot))
}

/// A bad body is the caller's mistake whatever is wrong with it (C1).
fn body_of(body: Result<Json<Change>, JsonRejection>) -> ApiResult<Change> {
    body.map(|Json(body)| body).map_err(|rejection| ApiError::bad_request(rejection.body_text()))
}

/// C15. `agents_of` leaves out a house bot from a house they've left, as `own_agent` refuses it (R44).
async fn wardrobe(State(state): State<AppState>, caller: Caller) -> ApiResult<Json<Value>> {
    ops::require_human(&caller, "open the wardrobe")?;
    let person = state.db.person(&caller.person_id)?.ok_or_else(ApiError::not_found)?;
    let agents = state.db.agents_of(&caller.person_id, now())?;
    let ids: Vec<String> = agents.iter().map(|agent| agent.id.clone()).collect();
    let mut yours = looks_of(&state, Wearer::Person, std::slice::from_ref(&person.id))?;
    let mut looks = looks_of(&state, Wearer::Bot, &ids)?;
    let mut bots_worn = worn(&state, Wearer::Bot, &ids)?;
    Ok(Json(json!({
        "parts": { "person": PERSON_PARTS, "bot": BOT_PARTS },
        "you": {
            "id": person.id,
            "name": person.name,
            "color": person.color,
            "look": yours.remove(&person.id).unwrap_or_default(),
            "worn": worn_by(&state, Wearer::Person, &person.id)?,
        },
        "bots": agents.iter().map(|agent| json!({
            "id": agent.id,
            "name": agent.name,
            "look": looks.remove(&agent.id).unwrap_or_default(),
            "worn": bots_worn.remove(&agent.id).unwrap_or_default(),
        })).collect::<Vec<_>>(),
    })))
}

/// C7, C8: who's asking comes before what they asked, so a bad body tells a bot nothing.
async fn change_own(
    State(state): State<AppState>,
    caller: Caller,
    body: Result<Json<Change>, JsonRejection>,
) -> ApiResult<Json<Value>> {
    ops::require_human(&caller, "change a look")?;
    slow_down(&state, &caller)?;
    let reply = change_look(&state, Wearer::Person, &caller.person_id, &body_of(body)?)?;
    refresh_person(&state, &caller.person_id)?;
    Ok(Json(reply))
}

/// C7, C8: only its owner, as a person. `own_agent` refuses every bot before it looks at the id.
async fn change_bot(
    State(state): State<AppState>,
    caller: Caller,
    Path(id): Path<String>,
    body: Result<Json<Change>, JsonRejection>,
) -> ApiResult<Json<Value>> {
    let agent = ops::own_agent(&state, &caller, &id, "change a look")?;
    slow_down(&state, &caller)?;
    let reply = change_look(&state, Wearer::Bot, &agent.id, &body_of(body)?)?;
    refresh_bot(&state, &agent)?;
    Ok(Json(reply))
}
