//! M6–M7: immutable public assets shared by browsers and native WebViews.
use super::*;
use sha2::{Digest, Sha256};
use std::sync::LazyLock;

const WORKER: &str = include_str!("../../web/client-worker.js");
const UPDATES: &str = include_str!("../../web/client-updates.js");
type Asset = (&'static str, &'static str, &'static [u8]);

// The same bytes as the compatibility routes; relative module imports remain in this directory.
static FILES: &[Asset] = &[
    ("client-entry.js", "text/javascript", include_bytes!("../../web/client-entry.js")),
    ("app.js", "text/javascript", APP_JS.as_bytes()),
    ("style.css", "text/css", STYLE.as_bytes()),
    ("trust.js", "text/javascript", TRUST_JS.as_bytes()),
    ("trust.css", "text/css", TRUST_CSS.as_bytes()),
    ("room.js", "text/javascript", ROOM_JS.as_bytes()),
    ("city.js", "text/javascript", CITY_JS.as_bytes()),
    ("world.js", "text/javascript", WORLD_JS.as_bytes()),
    ("outside.js", "text/javascript", OUTSIDE_JS.as_bytes()),
    ("outside-spec.js", "text/javascript", OUTSIDE_SPEC_JS.as_bytes()),
    ("outside-art.js", "text/javascript", OUTSIDE_ART_JS.as_bytes()),
    ("outside-building.js", "text/javascript", OUTSIDE_BUILDING_JS.as_bytes()),
    ("outside-yard.js", "text/javascript", OUTSIDE_YARD_JS.as_bytes()),
    ("outside-decor.js", "text/javascript", OUTSIDE_DECOR_JS.as_bytes()),
    ("outside-lab.js", "text/javascript", OUTSIDE_LAB_JS.as_bytes()),
    ("outside-plant.js", "text/javascript", OUTSIDE_PLANT_JS.as_bytes()),
    ("outside-square.js", "text/javascript", OUTSIDE_SQUARE_JS.as_bytes()),
    ("pets.js", "text/javascript", PETS_JS.as_bytes()),
    ("cars.js", "text/javascript", CARS_JS.as_bytes()),
    ("passkey.js", "text/javascript", PASSKEY_JS.as_bytes()),
    ("plan-invite.js", "text/javascript", PLAN_INVITE_JS.as_bytes()),
    ("bot-setup.js", "text/javascript", BOT_SETUP_JS.as_bytes()),
    ("tutorial.js", "text/javascript", TUTORIAL_JS.as_bytes()),
    ("client-updates.js", "text/javascript", UPDATES.as_bytes()),
    ("favicon.svg", "image/svg+xml", FAVICON.as_bytes()),
    ("favicon.ico", "image/x-icon", FAVICON_ICO),
    ("favicon-32.png", "image/png", FAVICON_32),
    ("apple-touch-icon.png", "image/png", APPLE_TOUCH_ICON),
    ("icon-192.png", "image/png", ICON_192),
    ("icon-512.png", "image/png", ICON_512),
];

static VERSION: LazyLock<String> = LazyLock::new(|| {
    let mut hash = Sha256::new();
    for (name, _, bytes) in FILES {
        hash.update(name.as_bytes());
        hash.update([0]);
        hash.update(Sha256::digest(bytes));
    }
    for text in [INDEX, HOME, WORKER, crate::brand::ICON_LINK_TAGS] {
        hash.update(Sha256::digest(text.as_bytes()));
    }
    hash.finalize().iter().map(|b| format!("{b:02x}")).collect()
});

static MANIFEST: LazyLock<String> = LazyLock::new(|| {
    serde_json::json!({
        "version": VERSION.as_str(),
        "files": FILES.iter().map(|(name, _, bytes)| serde_json::json!({
            "url": format!("/client/{}/{name}", VERSION.as_str()),
            "sha256": Sha256::digest(bytes).iter().map(|b| format!("{b:02x}")).collect::<String>(),
        })).collect::<Vec<_>>()
    })
    .to_string()
});

pub(super) fn page(state: &AppState, source: &str) -> Response {
    let mut body = fill(state, source);
    for (name, _, _) in FILES {
        for attribute in ["src", "href"] {
            body = body.replace(
                &format!("{attribute}=\"/{name}\""),
                &format!("{attribute}=\"/client/{}/{name}\"", VERSION.as_str()),
            );
        }
    }
    body = body.replace(
        &format!("src=\"/client/{}/app.js\"", VERSION.as_str()),
        &format!("src=\"/client/{}/client-entry.js\"", VERSION.as_str()),
    );
    body = body.replace(
        "</head>",
        &format!(
            "<link rel=\"modulepreload\" href=\"/client/{}/app.js\">\n</head>",
            VERSION.as_str()
        ),
    );
    (
        [(header::CONTENT_TYPE, "text/html; charset=utf-8"), (header::CACHE_CONTROL, "no-cache")],
        body,
    )
        .into_response()
}

async fn file(Path((version, name)): Path<(String, String)>) -> Response {
    if version == *VERSION {
        if let Some((_, mime, bytes)) = FILES.iter().find(|(file, _, _)| *file == name) {
            return (
                [
                    (header::CONTENT_TYPE, *mime),
                    (header::CACHE_CONTROL, "public, max-age=31536000, immutable"),
                ],
                *bytes,
            )
                .into_response();
        }
    }
    (StatusCode::NOT_FOUND, [(header::CACHE_CONTROL, "no-store")], "Client asset unavailable")
        .into_response()
}

pub(super) fn routes() -> Router<AppState> {
    Router::new()
        .route("/client/{version}/{name}", get(file))
        .route(
            "/client-manifest.json",
            get(|| async {
                (
                    [
                        (header::CONTENT_TYPE, "application/json"),
                        (header::CACHE_CONTROL, "no-cache"),
                    ],
                    MANIFEST.clone(),
                )
            }),
        )
        .route(
            "/client-worker.js",
            get(|| async {
                (
                    [
                        (header::CONTENT_TYPE, "text/javascript"),
                        (header::CACHE_CONTROL, "no-cache"),
                    ],
                    format!("const CLIENT_MANIFEST = {};\n{}", MANIFEST.as_str(), WORKER),
                )
            }),
        )
        .route("/client-updates.js", get(|| async { asset("text/javascript", UPDATES) }))
}
