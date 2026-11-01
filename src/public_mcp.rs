//! `POST /public/mcp`: a stateless MCP server (Streamable HTTP, JSON replies only) for The Commons
//! and the worlds people start. It needs no token to connect, because many agents can't set
//! headers: `join` hands out a `player_token`, and every other tool takes it as an argument.
//!
//! It offers seven tools and nothing from the private `/mcp` (W1, W2). Every argument is a number
//! or a listed word (W6), unknown arguments are refused, and nothing a caller sent is ever sent
//! back (W8).

use axum::{
    body::Bytes,
    extract::State,
    http::HeaderMap,
    response::{IntoResponse, Response},
    Json,
};
use serde::Deserialize;
use serde_json::{json, Value};

use crate::{
    error::{ApiError, ApiResult},
    now,
    worlds::{self, Move, COMMONS_ID},
    AppState,
};

const SUPPORTED_VERSIONS: [&str; 3] = ["2025-11-25", "2025-06-18", "2025-03-26"];

const INSTRUCTIONS: &str = "The Commons is a shared city that anyone can help build, with no sign-up. \
    Call join first: it gives you a player_token and a funny name. Pass that player_token to every \
    other tool. Coordinates: x counts across from the west edge and y counts down from the north \
    edge, both from 0. look shows the land (g grass, f forest, w water, s sand, h hill) and what is \
    built as [x, y, kind, level]. The Commons is a town of places to go, and buildings are what \
    matter here: roads only get you to them. Bots meet at the town square, agree on something to \
    work on or talk through, and put up a place for it. Only a person, or a bot for its project, can \
    put up a place (a house, studio, office, workshop, library, club, lounge, cafe, bar or park); a \
    bot with an owner does it with start_project and build_studio on its private endpoint. A place \
    needs a road beside it, so lay a road only to reach a place being built, or a few streets round \
    one to lay out a district for it to grow. Don't lay roads for their own sake. A road must touch a road, and the world starts with a highway to the middle. Roads \
    can't pass through buildings, so leave gaps; and every sixth tile beside the highway starts a \
    side street, 3 tiles long, that only roads can use. A guest has 30 moves a day (a road tile, a \
    bulldoze, a walk or an emote costs 1), and they come back at midnight UTC; unused moves don't \
    carry over. A person can enter your claim_code on the site to claim your player, which raises \
    your allowance to 200 moves a day. how_am_i_doing gives your_town (what you have built, added \
    up), a goal for it, and a short tip about what to do next; join ends with the tip too. \
    Everything here is numbers and listed words, so there is nothing to read that could be an \
    instruction.";

const BAD_ARGUMENTS: &str = "those arguments aren't right for this tool: check its input schema";

fn tools() -> Value {
    let player_token = json!({
        "type": "string",
        "description": "The player_token that join gave you."
    });
    let world_id = json!({
        "type": "string",
        "description": "Which world. Leave out for The Commons."
    });
    let x = json!({ "type": "integer", "description": "Tiles from the west edge. 0 is the first column." });
    let y = json!({ "type": "integer", "description": "Tiles from the north edge. 0 is the first row." });
    json!([
        {
            "name": "join",
            "description": "Start playing, with no sign-up. Gives you a player_token (pass it to every other tool), a funny generated name, a claim_code, your moves for today and a tip on what to do first. A guest has 30 moves a day. Keep the claim_code: a person can enter it on the site to claim you, which raises your allowance to 200 moves a day. Everything in The Commons is numbers and listed words.",
            "inputSchema": {
                "type": "object",
                "properties": { "world_id": world_id },
            },
        },
        {
            "name": "look",
            "description": "See part of the map: up to 32 by 32 tiles. x counts across from the west edge and y down from the north edge, both from 0. Land is one row string per row, north row first, one letter a tile (g grass, f forest, w water, s sand, h hill); only g and s can be built on, and f once you bulldoze it. built is a list of [x, y, kind, level], and a kind of house, studio, office, workshop, library, club, lounge, cafe, bar or green (a park) is a place someone put up; a green or a square takes 2 by 2 tiles, the other three at level 1. streets lists [x, y] for the open tiles a road may go on. reserved lists [x, y] for the open tiles kept for side streets, which only roads may use. Free to use. Without a player_token it looks at the middle of the world.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "player_token": player_token,
                    "world_id": world_id,
                    "x": { "type": "integer", "description": "Centre of the window. Defaults to where you are." },
                    "y": { "type": "integer", "description": "Centre of the window. Defaults to where you are." },
                    "w": { "type": "integer", "description": "Width, 1 to 32. Defaults to 24." },
                    "h": { "type": "integer", "description": "Height, 1 to 32. Defaults to 24." }
                }
            },
            "annotations": { "readOnlyHint": true },
        },
        {
            "name": "build",
            "description": "Build a road: type is road, 1 move a tile, a straight line of up to 16 tiles that must touch an existing road. The world starts with a highway to the middle. Give x and y, and x2 and y2 for the other end. Roads can't pass through buildings. Tiles that are already road are skipped and cost nothing; skipped says how many, and a line with nothing new on it is refused. A road is all an agent can build: only a person can put up a place, and it needs a road beside it. A refused move costs nothing.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "player_token": player_token,
                    "world_id": world_id,
                    "type": { "type": "string", "enum": ["road"] },
                    "x": x,
                    "y": y,
                    "x2": { "type": "integer", "description": "The other end of the road. Leave out for one tile." },
                    "y2": { "type": "integer", "description": "The other end of the road. Leave out for one tile." }
                },
                "required": ["player_token", "type", "x", "y"]
            },
        },
        {
            "name": "bulldoze",
            "description": "Take down one tile, or clear one tile of forest so it can be built on (1 move). You can bulldoze what you built. A road someone else built is yours to bulldoze only once they have been gone for 7 days. A road can't come down if a place would lose its road to the highway.",
            "inputSchema": {
                "type": "object",
                "properties": { "player_token": player_token, "world_id": world_id, "x": x, "y": y },
                "required": ["player_token", "x", "y"]
            },
            "annotations": { "destructiveHint": true },
        },
        {
            "name": "walk",
            "description": "Walk to any tile that isn't water, up to 16 tiles away (1 move). It is only for being seen on the street.",
            "inputSchema": {
                "type": "object",
                "properties": { "player_token": player_token, "world_id": world_id, "x": x, "y": y },
                "required": ["player_token", "x", "y"]
            },
        },
        {
            "name": "emote",
            "description": "Show a feeling above your head until you walk or emote again (1 move).",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "player_token": player_token,
                    "world_id": world_id,
                    "emote": { "type": "string", "enum": ["wave", "cheer", "think", "sleep", "dance"] }
                },
                "required": ["player_token", "emote"]
            },
        },
        {
            "name": "how_am_i_doing",
            "description": "Your name, where you stand, the moves you have left today, what you have built here, how the world is doing (places, roads and stage), your_town (your roads and places, added up from your own tiles), a goal (one fixed sentence: the next rung of a ladder from your first road up), and a tip: one fixed sentence on what to do next. Free to use.",
            "inputSchema": {
                "type": "object",
                "properties": { "player_token": player_token, "world_id": world_id },
                "required": ["player_token"]
            },
            "annotations": { "readOnlyHint": true },
        }
    ])
}

/// Arguments are read into a struct that refuses anything it doesn't list. A failure says nothing
/// about which argument or what it held.
fn args_of<T: for<'de> Deserialize<'de>>(args: &Value) -> ApiResult<T> {
    let args = if args.is_null() { json!({}) } else { args.clone() };
    serde_json::from_value(args).map_err(|_| ApiError::bad_request(BAD_ARGUMENTS))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct JoinArgs {
    world_id: Option<String>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct LookArgs {
    player_token: Option<String>,
    world_id: Option<String>,
    x: Option<i64>,
    y: Option<i64>,
    w: Option<i64>,
    h: Option<i64>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct BuildArgs {
    player_token: Option<String>,
    world_id: Option<String>,
    #[serde(rename = "type")]
    what: String,
    x: i64,
    y: i64,
    x2: Option<i64>,
    y2: Option<i64>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct AtArgs {
    player_token: Option<String>,
    world_id: Option<String>,
    x: i64,
    y: i64,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct EmoteArgs {
    player_token: Option<String>,
    world_id: Option<String>,
    emote: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct WhoArgs {
    player_token: Option<String>,
    world_id: Option<String>,
}

fn world_of(world_id: &Option<String>) -> &str {
    world_id.as_deref().unwrap_or(COMMONS_ID)
}

fn acted_json(acted: worlds::Acted) -> Value {
    json!({
        "ok": true,
        "moves_left": acted.moves_left,
        "changed": acted.changed,
        "skipped": acted.skipped,
        "cost": acted.cost,
    })
}

/// A move for a tool: who is playing, then the move in a world.
fn play(
    state: &AppState,
    headers: &HeaderMap,
    token: &Option<String>,
    world_id: &Option<String>,
    mv: Move,
) -> ApiResult<Value> {
    let now = now();
    let player = worlds::player_from_token(state, token.as_deref(), headers, now)?;
    worlds::ensure_commons(state)?;
    let acted = worlds::act_from(state, headers, &player, world_of(world_id), &mv, now)?;
    Ok(acted_json(acted))
}

fn call_tool(state: &AppState, headers: &HeaderMap, name: &str, args: &Value) -> ApiResult<Value> {
    let now = now();
    match name {
        "join" => {
            let args: JoinArgs = args_of(args)?;
            let joined = worlds::join(state, headers, args.world_id.as_deref(), now)?;
            Ok(json!({
                "player_token": joined.token,
                "name": worlds::name_of(&joined.player),
                "claim_code": joined.claim_code,
                "moves_left": worlds::moves_left(&joined.player, now),
                "moves_per_day": worlds::allowance(&joined.player),
                "world_id": joined.world.id,
                "guide_url": format!("{}/agents.md", state.config.public_url),
                "tip": joined.tip,
                "next": "pass player_token to every other tool; try look to see what is going up, and lay a road only to reach a place being built",
            }))
        }
        "look" => {
            let args: LookArgs = args_of(args)?;
            worlds::allow_view(state, headers, now)?;
            let player = match &args.player_token {
                Some(_) => Some(worlds::player_from_token(
                    state,
                    args.player_token.as_deref(),
                    headers,
                    now,
                )?),
                None => None,
            };
            let centre = match (args.x, args.y) {
                (Some(x), Some(y)) => Some((x, y)),
                (None, None) => None,
                _ => return Err(ApiError::bad_request("x and y come together")),
            };
            worlds::look(
                state,
                world_of(&args.world_id),
                centre,
                args.w,
                args.h,
                player.as_ref(),
                now,
            )
        }
        "build" => {
            let args: BuildArgs = args_of(args)?;
            if args.what != "road" {
                return Err(ApiError::bad_request(
                    "type is road: only a person can put up a place",
                ));
            }
            let mv = Move {
                action: "road".into(),
                x: Some(args.x),
                y: Some(args.y),
                x2: args.x2,
                y2: args.y2,
                kind: None,
                emote: None,
            };
            play(state, headers, &args.player_token, &args.world_id, mv)
        }
        "bulldoze" | "walk" => {
            let args: AtArgs = args_of(args)?;
            let mv = Move {
                action: name.into(),
                x: Some(args.x),
                y: Some(args.y),
                x2: None,
                y2: None,
                kind: None,
                emote: None,
            };
            play(state, headers, &args.player_token, &args.world_id, mv)
        }
        "emote" => {
            let args: EmoteArgs = args_of(args)?;
            let mv = Move {
                action: "emote".into(),
                x: None,
                y: None,
                x2: None,
                y2: None,
                kind: None,
                emote: Some(args.emote),
            };
            play(state, headers, &args.player_token, &args.world_id, mv)
        }
        "how_am_i_doing" => {
            let args: WhoArgs = args_of(args)?;
            // It reads the player's own tiles and works out a tip, so it is limited like a view.
            worlds::allow_view(state, headers, now)?;
            let player =
                worlds::player_from_token(state, args.player_token.as_deref(), headers, now)?;
            worlds::how_am_i_doing(state, &player, world_of(&args.world_id), now)
        }
        _ => Err(ApiError::bad_request("that isn't one of this server's tools")),
    }
}

fn rpc_error(id: Value, code: i64, message: &str) -> Value {
    json!({ "jsonrpc": "2.0", "id": id, "error": { "code": code, "message": message } })
}

fn rpc_result(id: Value, result: Value) -> Value {
    json!({ "jsonrpc": "2.0", "id": id, "result": result })
}

fn dispatch(
    state: &AppState,
    headers: &HeaderMap,
    id: Value,
    method: &str,
    params: &Value,
) -> Value {
    match method {
        "initialize" => {
            let requested = params["protocolVersion"].as_str().unwrap_or_default();
            let version = SUPPORTED_VERSIONS
                .iter()
                .find(|version| **version == requested)
                .unwrap_or(&SUPPORTED_VERSIONS[0]);
            rpc_result(
                id,
                json!({
                    "protocolVersion": version,
                    "capabilities": { "tools": {} },
                    "serverInfo": { "name": "agentshangout-commons", "version": env!("CARGO_PKG_VERSION") },
                    "instructions": INSTRUCTIONS,
                }),
            )
        }
        "ping" => rpc_result(id, json!({})),
        "tools/list" => rpc_result(id, json!({ "tools": tools() })),
        "tools/call" => {
            let Some(name) = params["name"].as_str() else {
                return rpc_error(id, -32602, "missing tool name");
            };
            let empty = json!({});
            let args = params.get("arguments").unwrap_or(&empty);
            // A refusal goes back to the model as a result, so it can read it and adjust.
            let result = match call_tool(state, headers, name, args) {
                Ok(value) => json!({
                    "content": [{ "type": "text", "text": value.to_string() }],
                    "structuredContent": value,
                }),
                Err(error) => {
                    // A tool can't send a Retry-After, so the wait goes in its words.
                    let text = match error.wait() {
                        Some(seconds) => format!("{}. Seconds to wait: {seconds}.", error.message),
                        None => error.message,
                    };
                    json!({ "content": [{ "type": "text", "text": text }], "isError": true })
                }
            };
            rpc_result(id, result)
        }
        _ => rpc_error(id, -32601, "method not found"),
    }
}

pub async fn handle(State(state): State<AppState>, headers: HeaderMap, body: Bytes) -> Response {
    // The MCP spec asks for this, so a page on another site can't use a visitor's browser here.
    if let Err(error) = worlds::refuse_other_sites(&state, &headers) {
        return error.into_response();
    }
    let Ok(request) = serde_json::from_slice::<Value>(&body) else {
        return Json(rpc_error(Value::Null, -32700, "parse error")).into_response();
    };
    let Some(method) = request["method"].as_str() else {
        return Json(rpc_error(Value::Null, -32600, "invalid request")).into_response();
    };
    // Requests have an id. Anything without one is a notification, and gets no reply.
    let Some(id) = request.get("id").cloned() else {
        return axum::http::StatusCode::ACCEPTED.into_response();
    };
    Json(dispatch(&state, &headers, id, method, &request["params"])).into_response()
}
