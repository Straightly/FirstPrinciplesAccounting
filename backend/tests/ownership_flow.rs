//! Two-party change-owner workflow: current-passphrase confirmation, frozen
//! pending state, successor acceptance, fresh encryption, and recovery.

use axum::body::Body;
use axum::http::{header, Method, Request, StatusCode};
use axum::Router;
use http_body_util::BodyExt;
use ledgerzero_backend::app::build_router;
use ledgerzero_backend::config::{DevLoginConfig, ServerConfig};
use ledgerzero_backend::state::AppState;
use serde_json::{json, Value};
use std::sync::Arc;
use tower::ServiceExt;
use uuid::Uuid;

const OWNER: &str = "owner@example.com";
const SUCCESSOR: &str = "successor@example.com";
const OLD_PASSPHRASE: &str = "old owner passphrase";
const NEW_PASSPHRASE: &str = "new owner passphrase";

fn app_over(books_dir: &std::path::Path) -> Router {
    let config = ServerConfig {
        listen_addr: "127.0.0.1:0".into(),
        books_dir: books_dir.to_string_lossy().into(),
        frontend_dist: "./nonexistent-dist".into(),
        dev_artifacts_dir: "./nonexistent-artifacts".into(),
        generated_workflows_dir: "./nonexistent-generated-workflows".into(),
        ops_audit_log: std::env::temp_dir()
            .join(format!("lz_owner_test_{}.jsonl", Uuid::new_v4()))
            .to_string_lossy()
            .into(),
        bootstrap_owner_email: OWNER.into(),
        session_ttl_seconds: 3600,
        secure_session_cookies: false,
        auth_providers: vec![],
        dev_login: DevLoginConfig { enabled: true },
    };
    build_router(Arc::new(AppState::new(config)))
}

async fn json_body(response: axum::response::Response) -> Value {
    let bytes = response.into_body().collect().await.unwrap().to_bytes();
    serde_json::from_slice(&bytes).unwrap()
}

async fn login(app: &Router, email: &str) -> String {
    let response = call(
        app,
        Method::POST,
        "/api/auth/dev-login",
        None,
        Some(json!({ "email": email })),
    )
    .await;
    response
        .headers()
        .get(header::SET_COOKIE)
        .unwrap()
        .to_str()
        .unwrap()
        .split(';')
        .next()
        .unwrap()
        .to_string()
}

async fn call(
    app: &Router,
    method: Method,
    uri: &str,
    cookie: Option<&str>,
    body: Option<Value>,
) -> axum::response::Response {
    let mut builder = Request::builder().method(method).uri(uri);
    if let Some(cookie) = cookie {
        builder = builder.header(header::COOKIE, cookie);
    }
    let request = if let Some(body) = body {
        builder
            .header(header::CONTENT_TYPE, "application/json")
            .body(Body::from(body.to_string()))
            .unwrap()
    } else {
        builder.body(Body::empty()).unwrap()
    };
    app.clone().oneshot(request).await.unwrap()
}

#[tokio::test]
async fn two_party_owner_transfer_freezes_then_reencrypts_for_the_successor() {
    let dir = tempfile::tempdir().unwrap();
    let app = app_over(dir.path());
    let old_cookie = login(&app, OWNER).await;
    let new_cookie = login(&app, SUCCESSOR).await;

    let created = call(
        &app,
        Method::POST,
        "/api/books",
        Some(&old_cookie),
        Some(json!({ "name": "Transfer me", "passphrase": OLD_PASSPHRASE })),
    )
    .await;
    assert_eq!(created.status(), StatusCode::OK);
    let created = json_body(created).await;
    let book_id = created["book_id"].as_str().unwrap();

    let wrong_confirmation = call(
        &app,
        Method::POST,
        &format!("/api/books/{book_id}/ownership-transfer/initiate"),
        Some(&old_cookie),
        Some(json!({
            "transfer_id": Uuid::new_v4(),
            "new_owner_email": SUCCESSOR,
            "current_passphrase": "not the current passphrase"
        })),
    )
    .await;
    assert_eq!(wrong_confirmation.status(), StatusCode::UNAUTHORIZED);

    let transfer_id = Uuid::new_v4();
    let initiated = call(
        &app,
        Method::POST,
        &format!("/api/books/{book_id}/ownership-transfer/initiate"),
        Some(&old_cookie),
        Some(json!({
            "transfer_id": transfer_id,
            "new_owner_email": SUCCESSOR,
            "current_passphrase": OLD_PASSPHRASE
        })),
    )
    .await;
    assert_eq!(initiated.status(), StatusCode::OK);
    let initiated = json_body(initiated).await;
    assert_eq!(initiated["owner_email"], OWNER);
    assert_eq!(
        initiated["pending_owner_transfer"]["new_owner_email"],
        SUCCESSOR
    );

    let successor_books = call(
        &app,
        Method::GET,
        "/api/books/mine",
        Some(&new_cookie),
        None,
    )
    .await;
    assert_eq!(successor_books.status(), StatusCode::OK);
    let successor_books = json_body(successor_books).await;
    assert_eq!(successor_books[0]["book_id"], book_id);
    assert_eq!(
        successor_books[0]["pending_owner_transfer"]["transfer_id"],
        transfer_id.to_string()
    );

    let frozen = call(
        &app,
        Method::GET,
        &format!("/api/books/{book_id}/entities"),
        Some(&old_cookie),
        None,
    )
    .await;
    assert_eq!(frozen.status(), StatusCode::CONFLICT);
    assert_eq!(
        json_body(frozen).await["error_code"],
        "BOOK_TRANSFER_PENDING"
    );

    let cannot_close = call(
        &app,
        Method::POST,
        &format!("/api/books/{book_id}/close"),
        Some(&old_cookie),
        None,
    )
    .await;
    assert_eq!(cannot_close.status(), StatusCode::CONFLICT);

    let wrong_identity = call(
        &app,
        Method::POST,
        &format!("/api/books/{book_id}/ownership-transfer/accept"),
        Some(&old_cookie),
        Some(json!({
            "op_id": Uuid::new_v4(),
            "new_passphrase": NEW_PASSPHRASE
        })),
    )
    .await;
    assert_eq!(wrong_identity.status(), StatusCode::FORBIDDEN);

    let acceptance_id = Uuid::new_v4();
    let accepted = call(
        &app,
        Method::POST,
        &format!("/api/books/{book_id}/ownership-transfer/accept"),
        Some(&new_cookie),
        Some(json!({
            "op_id": acceptance_id,
            "new_passphrase": NEW_PASSPHRASE
        })),
    )
    .await;
    assert_eq!(accepted.status(), StatusCode::OK);
    let accepted = json_body(accepted).await;
    assert_eq!(accepted["owner_email"], SUCCESSOR);
    assert!(accepted.get("pending_owner_transfer").is_none());

    let accepted_retry = call(
        &app,
        Method::POST,
        &format!("/api/books/{book_id}/ownership-transfer/accept"),
        Some(&new_cookie),
        Some(json!({
            "op_id": acceptance_id,
            "new_passphrase": NEW_PASSPHRASE
        })),
    )
    .await;
    assert_eq!(accepted_retry.status(), StatusCode::OK);

    let old_denied = call(
        &app,
        Method::GET,
        &format!("/api/books/{book_id}/entities"),
        Some(&old_cookie),
        None,
    )
    .await;
    assert_eq!(old_denied.status(), StatusCode::FORBIDDEN);

    let new_allowed = call(
        &app,
        Method::GET,
        &format!("/api/books/{book_id}/audit-log"),
        Some(&new_cookie),
        None,
    )
    .await;
    assert_eq!(new_allowed.status(), StatusCode::OK);
    let log = json_body(new_allowed).await;
    assert!(log.as_array().unwrap().iter().any(|event| {
        event["payload"]["kind"] == "book_owner_changed"
            && event["payload"]["new_owner_email"] == SUCCESSOR
    }));

    let closed = call(
        &app,
        Method::POST,
        &format!("/api/books/{book_id}/close"),
        Some(&new_cookie),
        None,
    )
    .await;
    assert_eq!(closed.status(), StatusCode::OK);

    let old_open_denied = call(
        &app,
        Method::POST,
        &format!("/api/books/{book_id}/open"),
        Some(&old_cookie),
        Some(json!({ "passphrase": OLD_PASSPHRASE })),
    )
    .await;
    assert_eq!(old_open_denied.status(), StatusCode::FORBIDDEN);

    let old_key_rejected = call(
        &app,
        Method::POST,
        &format!("/api/books/{book_id}/open"),
        Some(&new_cookie),
        Some(json!({ "passphrase": OLD_PASSPHRASE })),
    )
    .await;
    assert_eq!(old_key_rejected.status(), StatusCode::UNAUTHORIZED);

    let reopened = call(
        &app,
        Method::POST,
        &format!("/api/books/{book_id}/open"),
        Some(&new_cookie),
        Some(json!({ "passphrase": NEW_PASSPHRASE })),
    )
    .await;
    assert_eq!(reopened.status(), StatusCode::OK);

    let git_status = tokio::process::Command::new("git")
        .arg("-C")
        .arg(dir.path().join(book_id))
        .args(["status", "--porcelain"])
        .output()
        .await
        .unwrap();
    assert!(git_status.status.success());
    assert_eq!(
        String::from_utf8(git_status.stdout).unwrap(),
        "",
        "a successful owner transfer must checkpoint updated book.json"
    );
}

#[tokio::test]
async fn pending_transfer_survives_restart_and_requires_the_old_owner_to_resume() {
    let dir = tempfile::tempdir().unwrap();
    let book_id = {
        let app = app_over(dir.path());
        let owner_cookie = login(&app, OWNER).await;
        let created = call(
            &app,
            Method::POST,
            "/api/books",
            Some(&owner_cookie),
            Some(json!({ "name": "Restart transfer", "passphrase": OLD_PASSPHRASE })),
        )
        .await;
        let book_id = json_body(created).await["book_id"]
            .as_str()
            .unwrap()
            .to_string();
        let initiated = call(
            &app,
            Method::POST,
            &format!("/api/books/{book_id}/ownership-transfer/initiate"),
            Some(&owner_cookie),
            Some(json!({
                "transfer_id": Uuid::new_v4(),
                "new_owner_email": SUCCESSOR,
                "current_passphrase": OLD_PASSPHRASE
            })),
        )
        .await;
        assert_eq!(initiated.status(), StatusCode::OK);
        book_id
    };

    let app = app_over(dir.path());
    let owner_cookie = login(&app, OWNER).await;
    let successor_cookie = login(&app, SUCCESSOR).await;
    let listed = call(
        &app,
        Method::GET,
        "/api/books/mine",
        Some(&successor_cookie),
        None,
    )
    .await;
    let listed = json_body(listed).await;
    assert_eq!(listed[0]["book_id"], book_id);
    assert_eq!(listed[0]["is_open"], false);

    let premature = call(
        &app,
        Method::POST,
        &format!("/api/books/{book_id}/ownership-transfer/accept"),
        Some(&successor_cookie),
        Some(json!({ "op_id": Uuid::new_v4(), "new_passphrase": NEW_PASSPHRASE })),
    )
    .await;
    assert_eq!(premature.status(), StatusCode::CONFLICT);
    assert_eq!(json_body(premature).await["error_code"], "BOOK_NOT_OPEN");

    let resumed = call(
        &app,
        Method::POST,
        &format!("/api/books/{book_id}/open"),
        Some(&owner_cookie),
        Some(json!({ "passphrase": OLD_PASSPHRASE })),
    )
    .await;
    assert_eq!(resumed.status(), StatusCode::OK);
    let accepted = call(
        &app,
        Method::POST,
        &format!("/api/books/{book_id}/ownership-transfer/accept"),
        Some(&successor_cookie),
        Some(json!({ "op_id": Uuid::new_v4(), "new_passphrase": NEW_PASSPHRASE })),
    )
    .await;
    assert_eq!(accepted.status(), StatusCode::OK);
}

#[tokio::test]
async fn current_owner_can_cancel_a_mistaken_nomination_with_the_current_passphrase() {
    let dir = tempfile::tempdir().unwrap();
    let app = app_over(dir.path());
    let owner_cookie = login(&app, OWNER).await;
    let created = call(
        &app,
        Method::POST,
        "/api/books",
        Some(&owner_cookie),
        Some(json!({ "name": "Cancel transfer", "passphrase": OLD_PASSPHRASE })),
    )
    .await;
    let book_id = json_body(created).await["book_id"]
        .as_str()
        .unwrap()
        .to_string();
    let initiated = call(
        &app,
        Method::POST,
        &format!("/api/books/{book_id}/ownership-transfer/initiate"),
        Some(&owner_cookie),
        Some(json!({
            "transfer_id": Uuid::new_v4(),
            "new_owner_email": "mistyped@example.com",
            "current_passphrase": OLD_PASSPHRASE
        })),
    )
    .await;
    assert_eq!(initiated.status(), StatusCode::OK);

    let wrong = call(
        &app,
        Method::POST,
        &format!("/api/books/{book_id}/ownership-transfer/cancel"),
        Some(&owner_cookie),
        Some(json!({ "current_passphrase": "wrong passphrase value" })),
    )
    .await;
    assert_eq!(wrong.status(), StatusCode::UNAUTHORIZED);

    let cancelled = call(
        &app,
        Method::POST,
        &format!("/api/books/{book_id}/ownership-transfer/cancel"),
        Some(&owner_cookie),
        Some(json!({ "current_passphrase": OLD_PASSPHRASE })),
    )
    .await;
    assert_eq!(cancelled.status(), StatusCode::OK);
    assert!(json_body(cancelled)
        .await
        .get("pending_owner_transfer")
        .is_none());

    let operational = call(
        &app,
        Method::GET,
        &format!("/api/books/{book_id}/entities"),
        Some(&owner_cookie),
        None,
    )
    .await;
    assert_eq!(operational.status(), StatusCode::OK);
}
