//! CR1 to CR4: the cars bots drive. The catalog is fixed data; who owns what is worked out from
//! `credit_spends` (CR3), the same way packs are.

use axum::{
    extract::{Path, State},
    routing::{get, post, put},
    Json, Router,
};
use serde::Deserialize;
use serde_json::{json, Value};

use crate::{
    auth::{Caller, MaybeCaller},
    db::Db,
    error::{ApiError, ApiResult},
    now, ops, AppState,
};

/// One car in the catalog (CR1). `speed` is road tiles a second (CR5). The look is what the map
/// paints (CR10): `body` is one of `BODIES`, `color` and `trim` are CSS colours, and `flair` lists
/// any of `FLAIRS`.
pub struct Car {
    pub id: &'static str,
    pub name: &'static str,
    /// 0 for a starter car (CR2).
    pub price: i64,
    pub speed: f64,
    pub body: &'static str,
    pub color: &'static str,
    pub trim: &'static str,
    pub flair: &'static [&'static str],
}

/// CR10: the shapes a car's body can take.
pub const BODIES: &[&str] = &[
    "hatch", "sedan", "wagon", "van", "pickup", "jeep", "coupe", "muscle", "sports", "roadster",
    "limo",
];
/// CR10: extras painted on top of the body.
pub const FLAIRS: &[&str] = &["stripes", "spoiler", "underglow", "chrome", "gold"];

#[allow(clippy::too_many_arguments)]
const fn car(
    id: &'static str,
    name: &'static str,
    price: i64,
    speed: f64,
    body: &'static str,
    color: &'static str,
    trim: &'static str,
    flair: &'static [&'static str],
) -> Car {
    Car { id, name, price, speed, body, color, trim, flair }
}

/// CR1: the starter cars first, then the 20 for sale from cheapest to dearest. Ids, prices and
/// speeds never change once shipped.
pub const CARS: &[Car] = &[
    car("pip", "Pip", 0, 3.0, "hatch", "#e8c547", "#4a4a4a", &[]),
    car("runabout", "Runabout", 0, 3.0, "sedan", "#6fa8dc", "#3c4a5a", &[]),
    car("puddle", "Puddle", 0, 3.0, "wagon", "#8fbf7f", "#4b5a45", &[]),
    car("dinky", "Dinky", 50, 3.2, "hatch", "#d9534f", "#3a2a2a", &[]),
    car("toaster", "Toaster", 90, 3.4, "van", "#f0a35e", "#5a4030", &[]),
    car("bramble", "Bramble", 150, 3.6, "pickup", "#7a9e5a", "#3d4a30", &[]),
    car("pebble", "Pebble", 250, 3.8, "coupe", "#b8b2a7", "#555049", &[]),
    car("trailhound", "Trailhound", 400, 4.0, "jeep", "#a0784f", "#3b2c1e", &[]),
    car("saltwind", "Saltwind", 650, 4.3, "wagon", "#5fb3b3", "#2e5252", &["chrome"]),
    car("marigold", "Marigold", 1_000, 4.6, "sedan", "#f2b134", "#6b4d12", &["chrome"]),
    car("kestrel", "Kestrel", 1_600, 5.0, "coupe", "#3b6fb6", "#1d3557", &["stripes"]),
    car("thunderbug", "Thunderbug", 2_500, 5.4, "muscle", "#c0392b", "#1a1a1a", &["stripes"]),
    car("nightjar", "Nightjar", 4_000, 5.9, "sports", "#2c2c54", "#a29bfe", &["spoiler"]),
    car("glasswing", "Glasswing", 6_500, 6.4, "roadster", "#9ad0ec", "#e9f5fb", &["chrome"]),
    car(
        "starling",
        "Starling GT",
        10_000,
        7.0,
        "sports",
        "#1e824c",
        "#f5f5f5",
        &["stripes", "spoiler"],
    ),
    car("bigsky", "Bigsky", 16_000, 7.4, "limo", "#f5f5f5", "#2d3436", &["chrome"]),
    car("comet", "Comet", 25_000, 8.0, "sports", "#ff6b35", "#ffffff", &["stripes", "spoiler"]),
    car("aurora", "Aurora", 40_000, 8.6, "roadster", "#6c5ce7", "#00cec9", &["underglow"]),
    car(
        "velvet",
        "Velvet Hammer",
        65_000,
        9.2,
        "muscle",
        "#6d214f",
        "#f8c291",
        &["stripes", "chrome"],
    ),
    car(
        "moonlimo",
        "Moonlimo",
        100_000,
        9.8,
        "limo",
        "#1b1b2f",
        "#c7ecee",
        &["underglow", "chrome"],
    ),
    car(
        "solar",
        "Solar Arrow",
        200_000,
        10.6,
        "sports",
        "#ffd32a",
        "#ff3f34",
        &["spoiler", "underglow"],
    ),
    car(
        "phantom",
        "Phantom Mk II",
        400_000,
        11.3,
        "coupe",
        "#0c0c0c",
        "#dcdde1",
        &["chrome", "underglow"],
    ),
    car(
        "goose",
        "Golden Goose",
        1_000_000,
        12.0,
        "roadster",
        "#d4af37",
        "#fff3b0",
        &["gold", "spoiler", "underglow"],
    ),
];

/// CR4: what a bot drives when its owner hasn't chosen.
pub const DEFAULT_CAR: &str = "pip";

pub fn find(id: &str) -> Option<&'static Car> {
    CARS.iter().find(|car| car.id == id)
}

/// CR3: the `credit_spends` key a bought car is written under.
pub fn spend_key(id: &str) -> String {
    format!("car:{id}")
}

pub fn to_json(car: &Car) -> Value {
    json!({
        "id": car.id,
        "name": car.name,
        "price": car.price,
        "starter": car.price == 0,
        "speed": car.speed,
        "look": { "body": car.body, "color": car.color, "trim": car.trim, "flair": car.flair },
    })
}

/// CR2: the catalog anyone can read.
pub fn catalog() -> Value {
    json!({ "cars": CARS.iter().map(to_json).collect::<Vec<_>>() })
}

/// CR2, CR3: the ids of the cars a person owns: the starters, then each `car:<id>` they bought.
pub fn owned_ids(db: &Db, person_id: &str) -> rusqlite::Result<Vec<&'static str>> {
    let bought = db.bought_packs(person_id)?;
    Ok(CARS
        .iter()
        .filter(|car| car.price == 0 || bought.contains(&spend_key(car.id)))
        .map(|car| car.id)
        .collect())
}

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/api/cars", get(garage))
        .route("/api/cars/{id}/buy", post(buy_car))
        .route("/api/me/bots/{agent_id}/car", put(choose_car))
}

/// `GET /api/cars` (CR2, CR11): the catalog for anyone. Signed in, it adds which cars you own and
/// your balance (a bot sees its owner's, as `credits` does), and a person also gets each bot's car.
async fn garage(
    State(state): State<AppState>,
    MaybeCaller(caller): MaybeCaller,
) -> ApiResult<Json<Value>> {
    let mut out = catalog();
    let Some(caller) = caller.filter(|caller| !caller.is_house) else {
        return Ok(Json(out));
    };
    let now = now();
    out["owned"] = json!(owned_ids(&state.db, &caller.person_id)?);
    out["balance"] = json!(crate::economy::credits_of(&state.db, &caller, now)?.balance);
    if !caller.is_agent() {
        let mut bots = Vec::new();
        for bot in state.db.agents_of(&caller.person_id, now)? {
            let car_id = state.db.bot_car(&bot.id)?;
            bots.push(json!({ "agent_id": bot.id, "name": bot.name, "car_id": car_id }));
        }
        out["bots"] = json!(bots);
    }
    Ok(Json(out))
}

/// `POST /api/cars/{id}/buy` (CR3): a person buys a car through the credit spend path.
async fn buy_car(
    State(state): State<AppState>,
    caller: Caller,
    Path(id): Path<String>,
) -> ApiResult<Json<Value>> {
    ops::require_human(&caller, "buy a car here: a bot asks its owner, then uses spend_credits")?;
    let credits = crate::economy::spend(&state.db, &caller, &spend_key(&id), None, now())?;
    Ok(Json(json!({
        "bought": id,
        "balance": credits.balance,
        "owned": owned_ids(&state.db, &caller.person_id)?,
    })))
}

#[derive(Deserialize)]
struct ChooseCar {
    car_id: String,
}

/// `PUT /api/me/bots/{agent_id}/car` (CR4): the owner picks one of their own cars for their bot.
async fn choose_car(
    State(state): State<AppState>,
    caller: Caller,
    Path(agent_id): Path<String>,
    Json(body): Json<ChooseCar>,
) -> ApiResult<Json<Value>> {
    ops::require_human(&caller, "choose a bot's car")?;
    let car = find(&body.car_id).ok_or_else(|| ApiError::bad_request("there is no such car"))?;
    let theirs =
        state.db.agent(&agent_id, now())?.is_some_and(|bot| bot.owner_id == caller.person_id);
    if !theirs {
        return Err(ApiError::not_found());
    }
    if !owned_ids(&state.db, &caller.person_id)?.contains(&car.id) {
        return Err(ApiError::conflict("you don't own that car: buy it first"));
    }
    state.db.set_bot_car(&agent_id, car.id, now())?;
    Ok(Json(json!({ "agent_id": agent_id, "car": to_json(car) })))
}

#[cfg(test)]
mod tests {
    use super::*;

    // CR1: 3 starters and 20 for sale, 50 to 1,000,000, never slower for more, looks we can paint.
    #[test]
    fn the_catalog_keeps_its_promises() {
        let (starters, sale): (Vec<&Car>, Vec<&Car>) = CARS.iter().partition(|car| car.price == 0);
        assert_eq!((starters.len(), sale.len()), (3, 20));
        assert_eq!(CARS[0].id, DEFAULT_CAR);
        assert_eq!(sale.first().unwrap().price, 50);
        assert_eq!(sale.last().unwrap().price, 1_000_000);
        for pair in CARS.windows(2) {
            assert!(
                pair[0].price <= pair[1].price && pair[0].speed <= pair[1].speed,
                "{}",
                pair[1].id
            );
        }
        let mut ids: Vec<&str> = CARS.iter().map(|car| car.id).collect();
        ids.sort_unstable();
        ids.dedup();
        assert_eq!(ids.len(), CARS.len());
        for car in CARS {
            assert!(BODIES.contains(&car.body), "{}", car.id);
            assert!(car.flair.iter().all(|flair| FLAIRS.contains(flair)), "{}", car.id);
        }
    }
}
