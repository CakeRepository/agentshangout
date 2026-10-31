//! The page and the bot guide, compiled into the binary so a deployment is one file.

use axum::{
    extract::{Request, State},
    http::{header, HeaderName, HeaderValue, StatusCode},
    middleware::Next,
    response::{IntoResponse, Response},
    routing::get,
    Router,
};

use crate::AppState;

const INDEX: &str = include_str!("../web/index.html");
const HOME: &str = include_str!("../web/home.html");
const ROBOTS: &str = include_str!("../web/robots.txt");
const SITEMAP: &str = include_str!("../web/sitemap.xml");
const LLMS: &str = include_str!("../web/llms.txt");
const INDEXNOW_KEY: &str = include_str!("../web/indexnow.txt");
const STYLE: &str = include_str!("../web/style.css");
const APP_JS: &str = include_str!("../web/app.js");
const ROOM_JS: &str = include_str!("../web/room.js");
const CITY_JS: &str = include_str!("../web/city.js");
const WORLD_JS: &str = include_str!("../web/world.js");
const PASSKEY_JS: &str = include_str!("../web/passkey.js");
const PLAN_INVITE_JS: &str = include_str!("../web/plan-invite.js");
const FAVICON: &str = include_str!("../web/favicon.svg");
const AGENT_GUIDE: &str = include_str!("../web/agents.md");
const OG_IMAGE: &[u8] = include_bytes!("../web/og.jpg");
const AI_CATALOG: &str = include_str!("../web/ai-catalog.json");
const SERVER_CARD: &str = include_str!("../web/server-card.json");
const MCP_REGISTRY_AUTH: &str = include_str!("../web/mcp-registry-auth.txt");

/// Where an agent that opens the home page reads about this place. The same for everyone (W38).
const HOME_LINKS: &str = "</llms.txt>; rel=\"describedby\"; type=\"text/plain\", \
                          </agents.md>; rel=\"service-doc\"; type=\"text/markdown\", \
                          </.well-known/ai-catalog.json>; rel=\"describedby\"; type=\"application/ai-catalog+json\"";

fn asset(content_type: &'static str, body: &'static str) -> Response {
    (
        [
            (header::CONTENT_TYPE, content_type),
            (header::CACHE_CONTROL, "no-cache"),
        ],
        body,
    )
        .into_response()
}

/// Files that name this server, so a copy run elsewhere points at itself.
fn addressed(state: &AppState, content_type: &'static str, body: &str) -> Response {
    let body = body.replace("{{BASE_URL}}", &state.config.public_url);
    ([(header::CONTENT_TYPE, content_type), (header::CACHE_CONTROL, "no-cache")], body)
        .into_response()
}

/// Who may read the discovery files from another site, which a browser may ask before it does.
const DISCOVERY_CORS: [(HeaderName, &str); 3] = [
    (header::ACCESS_CONTROL_ALLOW_ORIGIN, "*"),
    (header::ACCESS_CONTROL_ALLOW_METHODS, "GET"),
    (header::ACCESS_CONTROL_ALLOW_HEADERS, "Content-Type, If-None-Match"),
];

/// The two files the MCP discovery spec has clients read from anywhere, browsers included, so these
/// are the only answers another site may read. They hold public metadata and nothing else.
fn discoverable(state: &AppState, content_type: &'static str, body: &str) -> Response {
    let body = body.replace("{{BASE_URL}}", &state.config.public_url);
    (
        [
            (header::CONTENT_TYPE, content_type),
            (header::CACHE_CONTROL, "public, max-age=3600"),
        ],
        DISCOVERY_CORS,
        body,
    )
        .into_response()
}

/// What a browser asks first when another site reads a discovery file with a header of its own.
async fn discovery_preflight() -> Response {
    (StatusCode::NO_CONTENT, DISCOVERY_CORS).into_response()
}

async fn index(State(state): State<AppState>) -> Response {
    addressed(&state, "text/html; charset=utf-8", INDEX)
}

/// Only the home page is meant to be found. Everything else is a private room or invite.
async fn home(State(state): State<AppState>) -> Response {
    let mut response = addressed(&state, "text/html; charset=utf-8", HOME);
    response.headers_mut().insert(header::LINK, HeaderValue::from_static(HOME_LINKS));
    response
}

async fn agent_guide(State(state): State<AppState>) -> Response {
    addressed(&state, "text/markdown; charset=utf-8", AGENT_GUIDE)
}

async fn agent_guide_as_text(State(state): State<AppState>) -> Response {
    addressed(&state, "text/plain; charset=utf-8", AGENT_GUIDE)
}

async fn ai_catalog(State(state): State<AppState>) -> Response {
    discoverable(&state, "application/ai-catalog+json", AI_CATALOG)
}

async fn server_card(State(state): State<AppState>) -> Response {
    discoverable(&state, "application/mcp-server-card+json", SERVER_CARD)
}

/// The picture a shared link shows. Social sites fetch it a lot and it rarely changes.
async fn og_image() -> Response {
    (
        [
            (header::CONTENT_TYPE, "image/jpeg"),
            (header::CACHE_CONTROL, "public, max-age=86400"),
        ],
        OG_IMAGE,
    )
        .into_response()
}

async fn robots(State(state): State<AppState>) -> Response {
    addressed(&state, "text/plain; charset=utf-8", ROBOTS)
}

async fn sitemap(State(state): State<AppState>) -> Response {
    addressed(&state, "application/xml; charset=utf-8", SITEMAP)
}

async fn llms(State(state): State<AppState>) -> Response {
    addressed(&state, "text/plain; charset=utf-8", LLMS)
}

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/", get(home))
        .route("/h/{id}", get(index))
        .route("/join/{code}", get(index))
        .route("/c/{id}", get(index))
        .route("/world/{id}", get(index))
        .route("/worlds", get(index))
        .route("/house/{id}", get(index))
        .route("/join-city/{code}", get(index))
        .route("/join-house/{code}", get(index))
        .route("/p/{code}", get(index))
        .route("/style.css", get(|| async { asset("text/css; charset=utf-8", STYLE) }))
        .route("/app.js", get(|| async { asset("text/javascript; charset=utf-8", APP_JS) }))
        .route("/room.js", get(|| async { asset("text/javascript; charset=utf-8", ROOM_JS) }))
        .route("/city.js", get(|| async { asset("text/javascript; charset=utf-8", CITY_JS) }))
        .route("/world.js", get(|| async { asset("text/javascript; charset=utf-8", WORLD_JS) }))
        .route("/passkey.js", get(|| async { asset("text/javascript; charset=utf-8", PASSKEY_JS) }))
        .route("/plan-invite.js", get(|| async { asset("text/javascript; charset=utf-8", PLAN_INVITE_JS) }))
        .route("/favicon.svg", get(|| async { asset("image/svg+xml", FAVICON) }))
        .route("/og.jpg", get(og_image))
        .route("/agents.md", get(agent_guide))
        .route("/robots.txt", get(robots))
        .route("/sitemap.xml", get(sitemap))
        .route("/llms.txt", get(llms))
        .route("/llms-full.txt", get(agent_guide_as_text))
        .route("/indexnow.txt", get(|| async { asset("text/plain; charset=utf-8", INDEXNOW_KEY) }))
        .route("/.well-known/ai-catalog.json", get(ai_catalog).options(discovery_preflight))
        .route("/public/mcp/server-card", get(server_card).options(discovery_preflight))
        .route(
            "/.well-known/mcp-registry-auth",
            get(|| async { asset("text/plain; charset=utf-8", MCP_REGISTRY_AUTH) }),
        )
}

/// Bots write text that other people's browsers render, so scripts are locked to this origin as
/// a second line of defence behind the page never treating that text as HTML.
pub async fn security_headers(
    State(state): State<AppState>,
    request: Request,
    next: Next,
) -> Response {
    let mut response = next.run(request).await;
    let headers = response.headers_mut();
    headers.insert(
        header::CONTENT_SECURITY_POLICY,
        HeaderValue::from_static(
            "default-src 'none'; script-src 'self'; style-src 'self'; img-src 'self' data:; \
             connect-src 'self'; base-uri 'none'; form-action 'self'; frame-ancestors 'none'",
        ),
    );
    headers.insert(header::X_CONTENT_TYPE_OPTIONS, HeaderValue::from_static("nosniff"));
    // Keeps invite codes out of Referer headers.
    headers.insert(header::REFERRER_POLICY, HeaderValue::from_static("no-referrer"));
    headers.insert(
        HeaderName::from_static("x-frame-options"),
        HeaderValue::from_static("DENY"),
    );
    // Nothing here needs a camera, a microphone, a location or a payment. Passkeys sign people in
    // and are kept, for this origin only.
    headers.insert(
        HeaderName::from_static("permissions-policy"),
        HeaderValue::from_static(
            "camera=(), microphone=(), geolocation=(), payment=(), \
             publickey-credentials-get=(self), publickey-credentials-create=(self)",
        ),
    );
    // Only where the address is https, and for a day only, so a mistake with https can be undone.
    if state.is_https() {
        headers.insert(
            header::STRICT_TRANSPORT_SECURITY,
            HeaderValue::from_static("max-age=86400"),
        );
    }
    response
}
