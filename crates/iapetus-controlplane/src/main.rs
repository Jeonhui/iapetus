//! The Control Plane token endpoints (PRD §8.4).
//!
//! A thin HTTP shell over `TokenService`: the interesting rules — the Project
//! Key check, the refresh cap, revocation — are in the library and tested
//! there. This wires them to the four routes §8.4 lists for tokens, plus the
//! JWKS the verifiers fetch (§8.1).
//!
//! What it deliberately does not do yet is provision Desktops or run the rest
//! of §8. This is the auth spine — issue, refresh, revoke, publish keys — which
//! is what lets the gateway stop trusting shared secrets.

use std::sync::Arc;

use axum::extract::State;
use axum::http::{HeaderMap, StatusCode};
use axum::response::IntoResponse;
use axum::routing::{get, post};
use axum::{Json, Router};
use ed25519_dalek::SigningKey;
use iapetus_auth::{ActorType, Issuer};
use iapetus_controlplane::{Control, IssueError, IssueRequest, TokenKind, TokenService};
use serde::{Deserialize, Serialize};

struct Cp {
    svc: TokenService,
}

#[tokio::main]
async fn main() {
    let bind = std::env::var("IAPETUS_CP_BIND").unwrap_or_else(|_| "0.0.0.0:8090".into());
    let project_key =
        std::env::var("IAPETUS_PROJECT_KEY").unwrap_or_else(|_| "sk_iap_live_dev".into());
    let iss = std::env::var("IAPETUS_ISS").unwrap_or_else(|_| "https://api.iapetus.dev".into());

    // A signing key from the environment, or a fixed development one. The dev
    // key is deterministic so the gateway can be handed the matching JWKS
    // without a key-exchange dance during local testing — and it says so, so a
    // deployment does not mistake it for a real one.
    let key = match std::env::var("IAPETUS_SIGNING_KEY") {
        Ok(b64) => {
            use base64::Engine;
            let bytes = base64::engine::general_purpose::URL_SAFE_NO_PAD
                .decode(b64)
                .expect("IAPETUS_SIGNING_KEY is not valid base64url");
            let arr: [u8; 32] = bytes.try_into().expect("signing key must be 32 bytes");
            SigningKey::from_bytes(&arr)
        }
        Err(_) => {
            eprintln!("no IAPETUS_SIGNING_KEY set; using the fixed development key");
            SigningKey::from_bytes(&[42u8; 32])
        }
    };

    let issuer = Issuer::new("k1", key, iss.clone());
    let cp = Arc::new(Cp { svc: TokenService::new(issuer, &project_key, iss) });

    // Print the JWKS entry the gateway needs, so `IAPETUS_JWKS` can be copied
    // straight across in a local setup.
    {
        use base64::Engine;
        let x = base64::engine::general_purpose::URL_SAFE_NO_PAD
            .encode(cp.svc.verifying_key().to_bytes());
        println!("gateway JWKS entry:  IAPETUS_JWKS={}:{x}", cp.svc.kid());
    }

    let app = Router::new()
        .route("/.well-known/jwks.json", get(jwks))
        .route("/v1/tokens", post(issue))
        .route("/v1/tokens/refresh", post(refresh))
        .route("/v1/tokens/revoke", post(revoke))
        .with_state(cp);

    let listener = tokio::net::TcpListener::bind(&bind).await.expect("bind");
    println!("control plane listening on http://{bind}");
    axum::serve(listener, app).await.expect("serve");
}

fn now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

async fn jwks(State(cp): State<Arc<Cp>>) -> impl IntoResponse {
    (
        [(axum::http::header::CONTENT_TYPE, "application/json")],
        cp.svc.jwks_json(),
    )
}

/// The Project Key travels in `Authorization: Bearer …`, never in the body,
/// so it does not land in request logs that capture bodies (§8.1).
fn project_key(headers: &HeaderMap) -> Option<String> {
    headers
        .get(axum::http::header::AUTHORIZATION)?
        .to_str()
        .ok()?
        .strip_prefix("Bearer ")
        .map(str::to_string)
}

#[derive(Deserialize)]
struct IssueBody {
    #[serde(rename = "type")]
    kind: String, // "agent" | "viewer"
    desktop_id: String,
    actor: ActorBody,
    #[serde(default)]
    scopes: Vec<String>,
    #[serde(default)]
    control: Option<String>, // "read" | "write", viewer only
    ttl_sec: Option<i64>,
}

#[derive(Deserialize)]
struct ActorBody {
    #[serde(rename = "type")]
    kind: String, // "agent" | "human"
    id: String,
}

#[derive(Serialize)]
struct TokenResponse {
    token: String,
    expires_in: i64,
}

#[derive(Serialize)]
struct ErrorBody {
    code: String,
    message: String,
}

fn err(status: StatusCode, code: &str, message: String) -> axum::response::Response {
    (status, Json(ErrorBody { code: code.into(), message })).into_response()
}

async fn issue(
    State(cp): State<Arc<Cp>>,
    headers: HeaderMap,
    Json(body): Json<IssueBody>,
) -> axum::response::Response {
    let Some(key) = project_key(&headers) else {
        return err(StatusCode::UNAUTHORIZED, "NO_PROJECT_KEY", "missing Bearer Project Key".into());
    };

    let kind = match body.kind.as_str() {
        "agent" => TokenKind::Agent,
        "viewer" => TokenKind::Viewer,
        other => {
            return err(StatusCode::BAD_REQUEST, "BAD_TYPE", format!("unknown token type `{other}`"))
        }
    };
    let actor_type = match body.actor.kind.as_str() {
        "human" => ActorType::Human,
        "agent" => ActorType::Agent,
        other => {
            return err(StatusCode::BAD_REQUEST, "BAD_ACTOR", format!("unknown actor type `{other}`"))
        }
    };
    let control = match body.control.as_deref() {
        Some("write") => Control::Write,
        // A viewer always starts able to request at most what it is issued; the
        // default is read, the safe floor (§8.1).
        _ => Control::Read,
    };
    let ttl = body.ttl_sec.unwrap_or(match kind {
        TokenKind::Agent => iapetus_controlplane::AGENT_TTL_SEC,
        TokenKind::Viewer => iapetus_controlplane::VIEWER_TTL_SEC,
    });

    let req = IssueRequest {
        kind,
        actor_type,
        actor_id: body.actor.id,
        project_id: "prj_dev".into(),
        desktop_ids: vec![body.desktop_id],
        scopes: body.scopes,
        control,
        ttl_sec: ttl,
    };

    match cp.svc.issue(&key, &req, now()) {
        Ok(token) => Json(TokenResponse { token, expires_in: ttl }).into_response(),
        Err(IssueError::BadProjectKey) => {
            err(StatusCode::UNAUTHORIZED, "BAD_PROJECT_KEY", "the Project Key is not recognized".into())
        }
        Err(e) => err(StatusCode::BAD_REQUEST, "ISSUE_FAILED", e.to_string()),
    }
}

#[derive(Deserialize)]
struct RefreshBody {
    token: String,
}

async fn refresh(
    State(cp): State<Arc<Cp>>,
    Json(body): Json<RefreshBody>,
) -> axum::response::Response {
    match cp.svc.refresh(&body.token, now()) {
        Ok(token) => Json(TokenResponse { token, expires_in: 0 }).into_response(),
        Err(IssueError::LifetimeExhausted) => err(
            StatusCode::FORBIDDEN,
            "TOKEN_EXPIRED",
            "this token has reached its total lifetime".into(),
        ),
        Err(e) => err(StatusCode::UNAUTHORIZED, "REFRESH_FAILED", e.to_string()),
    }
}

#[derive(Deserialize)]
struct RevokeBody {
    jti: String,
}

async fn revoke(
    State(cp): State<Arc<Cp>>,
    headers: HeaderMap,
    Json(body): Json<RevokeBody>,
) -> axum::response::Response {
    // Revocation needs the Project Key too — a leaked token must not be able to
    // revoke others.
    let Some(key) = project_key(&headers) else {
        return err(StatusCode::UNAUTHORIZED, "NO_PROJECT_KEY", "missing Bearer Project Key".into());
    };
    if !cp.svc.project_key_ok(&key) {
        return err(StatusCode::UNAUTHORIZED, "BAD_PROJECT_KEY", "the Project Key is not recognized".into());
    }
    cp.svc.revoke(&body.jti, now());
    StatusCode::NO_CONTENT.into_response()
}
