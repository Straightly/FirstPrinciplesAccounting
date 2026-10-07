//! M5 integration tests: workflow deployment, auto-roles, role assignment,
//! and workflow-scoped authorization on `post_entry`, driven over HTTP
//! (Impl Plan M5 exit criteria: "the workflow runs only via a valid
//! deployment and role assignment; authorization tests pass").

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

const OWNER: &str = "zhian.job@gmail.com";
const EMPLOYEE: &str = "employee@example.com";

fn test_config(books_dir: &std::path::Path, dev_artifacts_dir: &std::path::Path) -> ServerConfig {
    let audit_path = std::env::temp_dir().join(format!("lz_test_audit_{}.jsonl", Uuid::new_v4()));
    ServerConfig {
        listen_addr: "127.0.0.1:0".to_string(),
        books_dir: books_dir.to_string_lossy().to_string(),
        frontend_dist: "./nonexistent-dist".to_string(),
        dev_artifacts_dir: dev_artifacts_dir.to_string_lossy().to_string(),
        generated_workflows_dir: dev_artifacts_dir
            .join("persistent-generated")
            .to_string_lossy()
            .to_string(),
        ops_audit_log: audit_path.to_string_lossy().to_string(),
        bootstrap_owner_email: OWNER.to_string(),
        session_ttl_seconds: 3600,
        secure_session_cookies: false,
        auth_providers: vec![],
        dev_login: DevLoginConfig { enabled: true },
    }
}

fn app_over(books_dir: &std::path::Path, dev_artifacts_dir: &std::path::Path) -> Router {
    build_router(Arc::new(AppState::new(test_config(
        books_dir,
        dev_artifacts_dir,
    ))))
}

async fn body_json(response: axum::response::Response) -> Value {
    let bytes = response.into_body().collect().await.unwrap().to_bytes();
    serde_json::from_slice(&bytes).unwrap()
}

fn session_cookie(response: &axum::response::Response) -> String {
    response
        .headers()
        .get(header::SET_COOKIE)
        .expect("Set-Cookie present")
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

async fn get(app: &Router, uri: &str, cookie: &str) -> axum::response::Response {
    call(app, Method::GET, uri, Some(cookie), None).await
}

async fn post(app: &Router, uri: &str, cookie: &str, body: Value) -> axum::response::Response {
    call(app, Method::POST, uri, Some(cookie), Some(body)).await
}

async fn dev_login(app: &Router, email: &str) -> (String, Uuid) {
    let response = call(
        app,
        Method::POST,
        "/api/auth/dev-login",
        None,
        Some(json!({ "email": email })),
    )
    .await;
    assert_eq!(response.status(), StatusCode::OK);
    let cookie = session_cookie(&response);
    let body = body_json(response).await;
    let user_id = Uuid::parse_str(body["user"]["user_id"].as_str().unwrap()).unwrap();
    (cookie, user_id)
}

async fn create_book(app: &Router, cookie: &str) -> (Uuid, Uuid) {
    let response = post(
        app,
        "/api/books",
        cookie,
        json!({ "name": "Acme Books", "passphrase": "correct horse battery staple" }),
    )
    .await;
    assert_eq!(response.status(), StatusCode::OK);
    let body = body_json(response).await;
    (
        Uuid::parse_str(body["book_id"].as_str().unwrap()).unwrap(),
        Uuid::parse_str(body["entity_id"].as_str().unwrap()).unwrap(),
    )
}

async fn id_field(response: axum::response::Response) -> Uuid {
    let status = response.status();
    let body = body_json(response).await;
    assert_eq!(status, StatusCode::OK, "expected 200 OK: {body}");
    Uuid::parse_str(body["id"].as_str().unwrap()).unwrap()
}

#[tokio::test]
async fn book_owner_can_assign_role_to_self_but_other_user_cannot_self_grant() {
    let books_dir = tempfile::tempdir().unwrap();
    let artifacts_dir = tempfile::tempdir().unwrap();
    let app = app_over(books_dir.path(), artifacts_dir.path());
    let (owner_cookie, owner_id) = dev_login(&app, OWNER).await;
    let (book_id, entity_id) = create_book(&app, &owner_cookie).await;
    let role_id = id_field(
        post(
            &app,
            &format!("/api/books/{book_id}/roles"),
            &owner_cookie,
            json!({ "op_id": Uuid::new_v4(), "entity_id": entity_id,
                "name": "Importer", "description": null }),
        )
        .await,
    )
    .await;
    let path = format!("/api/books/{book_id}/roles/{role_id}/users");
    let (employee_cookie, _) = dev_login(&app, EMPLOYEE).await;
    let denied = post(
        &app,
        &path,
        &employee_cookie,
        json!({ "op_id": Uuid::new_v4(), "assign_to_self": true }),
    )
    .await;
    assert_eq!(denied.status(), StatusCode::FORBIDDEN);

    let op_id = Uuid::new_v4();
    let self_assignment = json!({ "op_id": op_id, "assign_to_self": true });
    let assigned = post(&app, &path, &owner_cookie, self_assignment.clone()).await;
    assert_eq!(assigned.status(), StatusCode::OK);
    let replay = post(&app, &path, &owner_cookie, self_assignment).await;
    assert_eq!(replay.status(), StatusCode::OK);
    let roles = body_json(
        get(
            &app,
            &format!("/api/books/{book_id}/roles?entity_id={entity_id}"),
            &owner_cookie,
        )
        .await,
    )
    .await;
    assert_eq!(roles[0]["assigned_user_ids"], json!([owner_id]));

    let ambiguous = post(
        &app,
        &path,
        &owner_cookie,
        json!({ "op_id": Uuid::new_v4(), "assign_to_self": true, "user_email": EMPLOYEE }),
    )
    .await;
    assert_eq!(ambiguous.status(), StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn delegated_entity_and_account_setup_requires_each_explicit_permission() {
    let books_dir = tempfile::tempdir().unwrap();
    let artifacts_dir = tempfile::tempdir().unwrap();
    let app = app_over(books_dir.path(), artifacts_dir.path());
    let (owner_cookie, _) = dev_login(&app, OWNER).await;
    let (book_id, subject_id, _, _) = setup_book(&app, &owner_cookie).await;
    let owner_charts = body_json(
        get(
            &app,
            &format!("/api/books/{book_id}/charts?entity_id={subject_id}"),
            &owner_cookie,
        )
        .await,
    )
    .await;
    let chart_id = Uuid::parse_str(owner_charts[0]["chart_id"].as_str().unwrap()).unwrap();
    let owner_resources = body_json(
        get(
            &app,
            &format!("/api/books/{book_id}/resource-types"),
            &owner_cookie,
        )
        .await,
    )
    .await;
    let usd = Uuid::parse_str(owner_resources[0]["resource_type_id"].as_str().unwrap()).unwrap();
    let (employee_cookie, employee_id) = dev_login(&app, EMPLOYEE).await;
    let entities = format!("/api/books/{book_id}/entities");
    let relations = format!("/api/books/{book_id}/entity-relationships");
    let accounts = format!("/api/books/{book_id}/accounts");
    let property_request =
        json!({"op_id": Uuid::new_v4(), "name": "Elm House", "category": "PROPERTY"});
    assert_eq!(
        get(&app, &entities, &employee_cookie).await.status(),
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        post(&app, &entities, &employee_cookie, property_request.clone())
            .await
            .status(),
        StatusCode::FORBIDDEN
    );

    let role_id = id_field(post(&app, &format!("/api/books/{book_id}/roles"), &owner_cookie,
        json!({"op_id": Uuid::new_v4(), "entity_id": subject_id, "name": "Entity preparer", "description": null,
            "permissions": ["list_entities", "create_entity", "create_entity_relationship", "list_charts", "list_resource_types", "list_accounts", "create_account"]})).await).await;
    assert_eq!(
        post(
            &app,
            &format!("/api/books/{book_id}/roles/{role_id}/users"),
            &owner_cookie,
            json!({"op_id": Uuid::new_v4(), "user_id": employee_id})
        )
        .await
        .status(),
        StatusCode::OK
    );
    let property =
        id_field(post(&app, &entities, &employee_cookie, property_request.clone()).await).await;
    assert_eq!(
        id_field(post(&app, &entities, &employee_cookie, property_request).await).await,
        property
    );
    assert_eq!(
        body_json(get(&app, &entities, &employee_cookie).await)
            .await
            .as_array()
            .unwrap()
            .len(),
        2
    );
    let relation = json!({"op_id": Uuid::new_v4(), "from_entity_id": subject_id,
        "to_entity_id": property, "kind": "OWNS", "effective_from": "2026-01-01", "effective_to": null});
    id_field(post(&app, &relations, &employee_cookie, relation).await).await;
    assert_eq!(
        body_json(get(&app, &relations, &employee_cookie).await)
            .await
            .as_array()
            .unwrap()
            .len(),
        1
    );
    assert_eq!(
        get(
            &app,
            &format!("/api/books/{book_id}/charts?entity_id={subject_id}"),
            &employee_cookie
        )
        .await
        .status(),
        StatusCode::OK
    );
    assert_eq!(
        get(
            &app,
            &format!("/api/books/{book_id}/resource-types"),
            &employee_cookie
        )
        .await
        .status(),
        StatusCode::OK
    );
    let account = json!({"op_id": Uuid::new_v4(), "chart_id": chart_id, "name": "Elm property cash",
        "code": null, "account_type": "ASSET", "resource_type_id": usd,
        "parent_account_id": null, "associated_entity_id": property,
        "validation_rules": {}, "metadata": {}});
    id_field(post(&app, &accounts, &employee_cookie, account).await).await;
    let bad = json!({"op_id": Uuid::new_v4(), "chart_id": chart_id, "name": "Bad property cash",
        "code": null, "account_type": "ASSET", "resource_type_id": usd,
        "parent_account_id": null, "associated_entity_id": Uuid::new_v4(),
        "validation_rules": {}, "metadata": {}});
    assert_eq!(
        post(&app, &accounts, &employee_cookie, bad).await.status(),
        StatusCode::BAD_REQUEST
    );
}

#[tokio::test]
async fn list_accounts_requires_owner_or_explicit_role_permission() {
    let books_dir = tempfile::tempdir().unwrap();
    let artifacts_dir = tempfile::tempdir().unwrap();
    let app = app_over(books_dir.path(), artifacts_dir.path());
    let (owner_cookie, _) = dev_login(&app, OWNER).await;
    let (book_id, entity_id, _, _) = setup_book(&app, &owner_cookie).await;
    let charts = body_json(
        get(
            &app,
            &format!("/api/books/{book_id}/charts?entity_id={entity_id}"),
            &owner_cookie,
        )
        .await,
    )
    .await;
    let chart_id = charts[0]["chart_id"].as_str().unwrap();
    let accounts_path = format!("/api/books/{book_id}/accounts?chart_id={chart_id}");
    let (employee_cookie, employee_id) = dev_login(&app, EMPLOYEE).await;
    assert_eq!(
        get(&app, &accounts_path, &employee_cookie).await.status(),
        StatusCode::FORBIDDEN
    );

    let unrelated_role = id_field(
        post(
            &app,
            &format!("/api/books/{book_id}/roles"),
            &owner_cookie,
            json!({ "op_id": Uuid::new_v4(), "entity_id": entity_id,
                "name": "Other workflow role", "description": null }),
        )
        .await,
    )
    .await;
    let unrelated_assignment = post(
        &app,
        &format!("/api/books/{book_id}/roles/{unrelated_role}/users"),
        &owner_cookie,
        json!({ "op_id": Uuid::new_v4(), "user_id": employee_id }),
    )
    .await;
    assert_eq!(unrelated_assignment.status(), StatusCode::OK);
    assert_eq!(
        get(&app, &accounts_path, &employee_cookie).await.status(),
        StatusCode::FORBIDDEN
    );

    let role_id = id_field(
        post(
            &app,
            &format!("/api/books/{book_id}/roles"),
            &owner_cookie,
            json!({ "op_id": Uuid::new_v4(), "entity_id": entity_id,
                "name": "Account reader", "description": null,
                "permissions": ["list_accounts"] }),
        )
        .await,
    )
    .await;
    let assigned = post(
        &app,
        &format!("/api/books/{book_id}/roles/{role_id}/users"),
        &owner_cookie,
        json!({ "op_id": Uuid::new_v4(), "user_id": employee_id }),
    )
    .await;
    assert_eq!(assigned.status(), StatusCode::OK);
    let accounts = body_json(get(&app, &accounts_path, &employee_cookie).await).await;
    assert_eq!(accounts.as_array().unwrap().len(), 2);

    let invalid = post(
        &app,
        &format!("/api/books/{book_id}/roles"),
        &owner_cookie,
        json!({ "op_id": Uuid::new_v4(), "entity_id": entity_id,
            "name": "Unsafe role", "description": null,
            "permissions": ["delete_book"] }),
    )
    .await;
    assert_eq!(invalid.status(), StatusCode::BAD_REQUEST);
}

/// Writes a minimal, valid dev artifact to disk so `hash_artifact` succeeds.
fn write_artifact(dev_artifacts_dir: &std::path::Path, deployment_id: Uuid) {
    let dir = dev_artifacts_dir
        .join("workflows")
        .join(deployment_id.to_string());
    let code_dir = dir.join("code");
    std::fs::create_dir_all(&code_dir).unwrap();
    std::fs::write(
        dir.join("manifest.json"),
        format!(r#"{{"generator":"hand-written","workflow_deployment_id":"{deployment_id}"}}"#),
    )
    .unwrap();
    std::fs::write(
        code_dir.join("index.html"),
        "<!doctype html><div id=\"root\"></div>",
    )
    .unwrap();
    std::fs::write(code_dir.join("app.js"), "// hand-written workflow app").unwrap();
}

/// Sets up a book with one entity, a USD asset/expense account pair, and an
/// open period — everything `post_entry` needs — returning the ids used.
async fn setup_book(app: &Router, owner_cookie: &str) -> (Uuid, Uuid, Uuid, Uuid) {
    let (book_id, entity_id) = create_book(app, owner_cookie).await;
    let usd = id_field(
        post(
            app,
            &format!("/api/books/{book_id}/resource-types"),
            owner_cookie,
            json!({
                "op_id": Uuid::new_v4(), "name": "US Dollar", "kind": "CURRENCY",
                "code": "USD", "unit_of_measure": "USD", "precision": 2
            }),
        )
        .await,
    )
    .await;
    let chart_id = id_field(
        post(
            app,
            &format!("/api/books/{book_id}/charts"),
            owner_cookie,
            json!({ "op_id": Uuid::new_v4(), "entity_id": entity_id, "name": "Main", "description": null, "activate": true }),
        )
        .await,
    )
    .await;
    let make_account = |name: &'static str, account_type: &'static str| {
        let app = app.clone();
        let owner_cookie = owner_cookie.to_string();
        async move {
            id_field(
                post(
                    &app,
                    &format!("/api/books/{book_id}/accounts"),
                    &owner_cookie,
                    json!({
                        "op_id": Uuid::new_v4(), "chart_id": chart_id, "name": name, "code": null,
                        "account_type": account_type, "resource_type_id": usd,
                        "parent_account_id": null, "validation_rules": null, "metadata": null
                    }),
                )
                .await,
            )
            .await
        }
    };
    let cash = make_account("Cash", "ASSET").await;
    let rent = make_account("Rent Expense", "EXPENSE").await;
    let period_resp = post(
        app,
        &format!("/api/books/{book_id}/periods"),
        owner_cookie,
        json!({
            "op_id": Uuid::new_v4(), "entity_id": entity_id, "name": "2026-02",
            "start_date": "2026-02-01", "end_date": "2026-02-28"
        }),
    )
    .await;
    assert_eq!(period_resp.status(), StatusCode::OK);
    (book_id, entity_id, cash, rent)
}

#[tokio::test]
async fn full_workflow_lifecycle_over_http() {
    let books_dir = tempfile::tempdir().unwrap();
    let artifacts_dir = tempfile::tempdir().unwrap();
    let app = app_over(books_dir.path(), artifacts_dir.path());
    let (owner_cookie, _owner_id) = dev_login(&app, OWNER).await;
    let (book_id, entity_id, cash, rent) = setup_book(&app, &owner_cookie).await;

    let deployment_id = Uuid::new_v4();
    write_artifact(artifacts_dir.path(), deployment_id);

    let deploy_resp = post(
        &app,
        &format!("/api/books/{book_id}/workflows/deploy"),
        &owner_cookie,
        json!({
            "workflow_deployment_id": deployment_id,
            "workflow_id": Uuid::new_v4(),
            "entity_id": entity_id,
            "workflow_name": "Recording startup expense",
            "description": "Hand-built reference workflow",
            "backend_api_calls": ["post_entry"]
        }),
    )
    .await;
    assert_eq!(id_field(deploy_resp).await, deployment_id);

    // Admin view: the deployment and its auto-role both exist.
    let workflows = body_json(
        get(
            &app,
            &format!("/api/books/{book_id}/workflows?entity_id={entity_id}"),
            &owner_cookie,
        )
        .await,
    )
    .await;
    let workflows = workflows.as_array().unwrap();
    assert_eq!(workflows.len(), 1);
    assert_eq!(workflows[0]["workflow_name"], "Recording startup expense");
    let workflow_id = Uuid::parse_str(workflows[0]["workflow_id"].as_str().unwrap()).unwrap();

    let roles = body_json(
        get(
            &app,
            &format!("/api/books/{book_id}/roles?entity_id={entity_id}"),
            &owner_cookie,
        )
        .await,
    )
    .await;
    let roles = roles.as_array().unwrap();
    assert_eq!(roles.len(), 1, "deploying a workflow auto-creates one role");
    let role_id = Uuid::parse_str(roles[0]["role_id"].as_str().unwrap()).unwrap();

    // Assign the auto-role to an employee — not the book owner.
    let (employee_cookie, employee_id) = dev_login(&app, EMPLOYEE).await;
    let assign_resp = post(
        &app,
        &format!("/api/books/{book_id}/roles/{role_id}/users"),
        &owner_cookie,
        json!({ "op_id": Uuid::new_v4(), "user_id": employee_id }),
    )
    .await;
    assert_eq!(assign_resp.status(), StatusCode::OK);

    // The employee's launcher menu now shows the workflow.
    let mine = body_json(
        get(
            &app,
            &format!("/api/books/{book_id}/workflows/mine?entity_id={entity_id}"),
            &employee_cookie,
        )
        .await,
    )
    .await;
    assert_eq!(mine.as_array().unwrap().len(), 1);

    // The employee runs the workflow: posts a balanced entry carrying full
    // execution context, authorized purely by role assignment (no
    // Action::BookApi / bootstrap-owner check on this path).
    let execution_id = Uuid::new_v4();
    let entry_resp = post(
        &app,
        &format!("/api/books/{book_id}/entries"),
        &employee_cookie,
        json!({
            "entry_id": Uuid::new_v4(), "entity_id": entity_id, "entry_date": "2026-02-10",
            "description": "startup laptop expense", "source": "WORKFLOW",
            "workflow": {
                "workflow_id": workflow_id,
                "workflow_deployment_id": deployment_id,
                "workflow_execution_id": execution_id
            },
            "lines": [
                { "line_id": Uuid::new_v4(), "account_id": rent, "debit_amount": "899.00", "credit_amount": null, "memo": null },
                { "line_id": Uuid::new_v4(), "account_id": cash, "debit_amount": null, "credit_amount": "899.00", "memo": null }
            ]
        }),
    )
    .await;
    assert_eq!(
        entry_resp.status(),
        StatusCode::OK,
        "{:?}",
        body_json(entry_resp).await
    );

    let balance = body_json(
        get(
            &app,
            &format!("/api/books/{book_id}/accounts/{rent}/balance"),
            &owner_cookie,
        )
        .await,
    )
    .await;
    assert_eq!(balance["debit_total"], "899.00000000");
}

#[tokio::test]
async fn workflow_scoped_post_entry_rejects_unassigned_users() {
    let books_dir = tempfile::tempdir().unwrap();
    let artifacts_dir = tempfile::tempdir().unwrap();
    let app = app_over(books_dir.path(), artifacts_dir.path());
    let (owner_cookie, _) = dev_login(&app, OWNER).await;
    let (book_id, entity_id, cash, rent) = setup_book(&app, &owner_cookie).await;

    let deployment_id = Uuid::new_v4();
    write_artifact(artifacts_dir.path(), deployment_id);
    post(
        &app,
        &format!("/api/books/{book_id}/workflows/deploy"),
        &owner_cookie,
        json!({
            "workflow_deployment_id": deployment_id, "workflow_id": Uuid::new_v4(),
            "entity_id": entity_id,
            "workflow_name": "Recording startup expense", "description": null,
            "backend_api_calls": ["post_entry"]
        }),
    )
    .await;
    let workflows = body_json(
        get(
            &app,
            &format!("/api/books/{book_id}/workflows?entity_id={entity_id}"),
            &owner_cookie,
        )
        .await,
    )
    .await;
    let workflow_id = Uuid::parse_str(workflows[0]["workflow_id"].as_str().unwrap()).unwrap();

    // A signed-in but unassigned user (not even the owner is assigned this
    // role) attempts to run the workflow directly.
    let (stranger_cookie, _) = dev_login(&app, "stranger@example.com").await;
    let entry_resp = post(
        &app,
        &format!("/api/books/{book_id}/entries"),
        &stranger_cookie,
        json!({
            "entry_id": Uuid::new_v4(), "entity_id": entity_id, "entry_date": "2026-02-10",
            "description": "unauthorized attempt", "source": "WORKFLOW",
            "workflow": {
                "workflow_id": workflow_id,
                "workflow_deployment_id": deployment_id,
                "workflow_execution_id": Uuid::new_v4()
            },
            "lines": [
                { "line_id": Uuid::new_v4(), "account_id": rent, "debit_amount": "10.00", "credit_amount": null, "memo": null },
                { "line_id": Uuid::new_v4(), "account_id": cash, "debit_amount": null, "credit_amount": "10.00", "memo": null }
            ]
        }),
    )
    .await;
    assert_eq!(entry_resp.status(), StatusCode::FORBIDDEN);
    let err = body_json(entry_resp).await;
    assert_eq!(err["error_code"], "UNAUTHORIZED_WORKFLOW");
}

#[tokio::test]
async fn workflow_scoped_post_entry_rejects_disallowed_api_and_bad_context() {
    let books_dir = tempfile::tempdir().unwrap();
    let artifacts_dir = tempfile::tempdir().unwrap();
    let app = app_over(books_dir.path(), artifacts_dir.path());
    let (owner_cookie, _) = dev_login(&app, OWNER).await;
    let (book_id, entity_id, cash, rent) = setup_book(&app, &owner_cookie).await;

    // Deploy a workflow whose allow-list does NOT include post_entry.
    let deployment_id = Uuid::new_v4();
    write_artifact(artifacts_dir.path(), deployment_id);
    post(
        &app,
        &format!("/api/books/{book_id}/workflows/deploy"),
        &owner_cookie,
        json!({
            "workflow_deployment_id": deployment_id, "workflow_id": Uuid::new_v4(),
            "entity_id": entity_id,
            "workflow_name": "Read-only report", "description": null,
            "backend_api_calls": ["get_balance"]
        }),
    )
    .await;
    let workflows = body_json(
        get(
            &app,
            &format!("/api/books/{book_id}/workflows?entity_id={entity_id}"),
            &owner_cookie,
        )
        .await,
    )
    .await;
    let workflow_id = Uuid::parse_str(workflows[0]["workflow_id"].as_str().unwrap()).unwrap();
    let roles = body_json(
        get(
            &app,
            &format!("/api/books/{book_id}/roles?entity_id={entity_id}"),
            &owner_cookie,
        )
        .await,
    )
    .await;
    let role_id = Uuid::parse_str(roles[0]["role_id"].as_str().unwrap()).unwrap();
    let (employee_cookie, employee_id) = dev_login(&app, EMPLOYEE).await;
    post(
        &app,
        &format!("/api/books/{book_id}/roles/{role_id}/users"),
        &owner_cookie,
        json!({ "op_id": Uuid::new_v4(), "user_id": employee_id }),
    )
    .await;

    let disallowed_resp = post(
        &app,
        &format!("/api/books/{book_id}/entries"),
        &employee_cookie,
        json!({
            "entry_id": Uuid::new_v4(), "entity_id": entity_id, "entry_date": "2026-02-10",
            "description": "not permitted", "source": "WORKFLOW",
            "workflow": {
                "workflow_id": workflow_id,
                "workflow_deployment_id": deployment_id,
                "workflow_execution_id": Uuid::new_v4()
            },
            "lines": [
                { "line_id": Uuid::new_v4(), "account_id": rent, "debit_amount": "5.00", "credit_amount": null, "memo": null },
                { "line_id": Uuid::new_v4(), "account_id": cash, "debit_amount": null, "credit_amount": "5.00", "memo": null }
            ]
        }),
    )
    .await;
    assert_eq!(disallowed_resp.status(), StatusCode::FORBIDDEN);
    assert_eq!(
        body_json(disallowed_resp).await["error_code"],
        "UNAUTHORIZED_API"
    );

    // Unknown workflow_deployment_id: INVALID_EXECUTION_CONTEXT.
    let bad_context_resp = post(
        &app,
        &format!("/api/books/{book_id}/entries"),
        &employee_cookie,
        json!({
            "entry_id": Uuid::new_v4(), "entity_id": entity_id, "entry_date": "2026-02-10",
            "description": "bad context", "source": "WORKFLOW",
            "workflow": {
                "workflow_id": workflow_id,
                "workflow_deployment_id": Uuid::new_v4(),
                "workflow_execution_id": Uuid::new_v4()
            },
            "lines": [
                { "line_id": Uuid::new_v4(), "account_id": rent, "debit_amount": "5.00", "credit_amount": null, "memo": null },
                { "line_id": Uuid::new_v4(), "account_id": cash, "debit_amount": null, "credit_amount": "5.00", "memo": null }
            ]
        }),
    )
    .await;
    assert_eq!(bad_context_resp.status(), StatusCode::BAD_REQUEST);
    assert_eq!(
        body_json(bad_context_resp).await["error_code"],
        "INVALID_EXECUTION_CONTEXT"
    );
}

#[tokio::test]
async fn deploy_workflow_requires_bootstrap_owner() {
    let books_dir = tempfile::tempdir().unwrap();
    let artifacts_dir = tempfile::tempdir().unwrap();
    let app = app_over(books_dir.path(), artifacts_dir.path());
    let (owner_cookie, _) = dev_login(&app, OWNER).await;
    let (book_id, entity_id, _, _) = setup_book(&app, &owner_cookie).await;

    let deployment_id = Uuid::new_v4();
    write_artifact(artifacts_dir.path(), deployment_id);
    let (other_cookie, _) = dev_login(&app, "someone.else@example.com").await;
    let response = post(
        &app,
        &format!("/api/books/{book_id}/workflows/deploy"),
        &other_cookie,
        json!({
            "workflow_deployment_id": deployment_id, "workflow_id": Uuid::new_v4(),
            "entity_id": entity_id,
            "workflow_name": "Recording startup expense", "description": null,
            "backend_api_calls": ["post_entry"]
        }),
    )
    .await;
    assert_eq!(response.status(), StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn deploy_workflow_fails_when_artifact_is_missing_from_disk() {
    let books_dir = tempfile::tempdir().unwrap();
    let artifacts_dir = tempfile::tempdir().unwrap();
    let app = app_over(books_dir.path(), artifacts_dir.path());
    let (owner_cookie, _) = dev_login(&app, OWNER).await;
    let (book_id, entity_id, _, _) = setup_book(&app, &owner_cookie).await;

    // No write_artifact() call — nothing on disk for this deployment id.
    let response = post(
        &app,
        &format!("/api/books/{book_id}/workflows/deploy"),
        &owner_cookie,
        json!({
            "workflow_deployment_id": Uuid::new_v4(), "workflow_id": Uuid::new_v4(),
            "entity_id": entity_id,
            "workflow_name": "Recording startup expense", "description": null,
            "backend_api_calls": ["post_entry"]
        }),
    )
    .await;
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
}

/// M6/M7: the launcher's book picker. Deploys a workflow and assigns its
/// auto-role to an employee, then checks `GET /books/mine` (each book now
/// carries its one entity_id directly, Impl Plan M7) — where the bootstrap
/// owner sees every book regardless of role assignment — and
/// `GET /books/:id/workflows/mine`, which is scoped purely by role
/// assignment even for the owner: the deploying owner holds no role here,
/// the assigned employee sees exactly the one workflow their role grants,
/// and an unassigned stranger sees nothing, not an error.
#[tokio::test]
async fn book_and_entity_picker_scopes_by_role_assignment() {
    let books_dir = tempfile::tempdir().unwrap();
    let artifacts_dir = tempfile::tempdir().unwrap();
    let app = app_over(books_dir.path(), artifacts_dir.path());
    let (owner_cookie, _) = dev_login(&app, OWNER).await;
    let (book_id, entity_id, _, _) = setup_book(&app, &owner_cookie).await;

    let deployment_id = Uuid::new_v4();
    write_artifact(artifacts_dir.path(), deployment_id);
    post(
        &app,
        &format!("/api/books/{book_id}/workflows/deploy"),
        &owner_cookie,
        json!({
            "workflow_deployment_id": deployment_id, "workflow_id": Uuid::new_v4(),
            "entity_id": entity_id,
            "workflow_name": "Recording startup expense", "description": null,
            "backend_api_calls": ["post_entry"]
        }),
    )
    .await;
    let role_id = {
        let roles = body_json(
            get(
                &app,
                &format!("/api/books/{book_id}/roles?entity_id={entity_id}"),
                &owner_cookie,
            )
            .await,
        )
        .await;
        Uuid::parse_str(roles[0]["role_id"].as_str().unwrap()).unwrap()
    };
    let (employee_cookie, employee_id) = dev_login(&app, EMPLOYEE).await;
    post(
        &app,
        &format!("/api/books/{book_id}/roles/{role_id}/users"),
        &owner_cookie,
        json!({ "op_id": Uuid::new_v4(), "user_id": employee_id }),
    )
    .await;
    let (stranger_cookie, _) = dev_login(&app, "stranger@example.com").await;

    // Owner: /books/mine matches /books exactly (every book on disk), and
    // the book already carries its one entity_id (Impl Plan M7).
    let owner_books = body_json(get(&app, "/api/books/mine", &owner_cookie).await).await;
    let owner_books = owner_books.as_array().unwrap();
    assert_eq!(owner_books.len(), 1);
    assert_eq!(owner_books[0]["entity_id"], entity_id.to_string());
    let owner_workflows = body_json(
        get(
            &app,
            &format!("/api/books/{book_id}/workflows/mine?entity_id={entity_id}"),
            &owner_cookie,
        )
        .await,
    )
    .await;
    assert_eq!(
        owner_workflows.as_array().unwrap().len(),
        0,
        "workflows/mine is role-scoped even for the bootstrap owner"
    );

    // Employee: sees exactly the one book they were assigned into, with the
    // one workflow their role grants.
    let employee_books = body_json(get(&app, "/api/books/mine", &employee_cookie).await).await;
    let employee_books = employee_books.as_array().unwrap();
    assert_eq!(employee_books.len(), 1);
    assert_eq!(employee_books[0]["book_id"], book_id.to_string());
    assert_eq!(employee_books[0]["entity_id"], entity_id.to_string());
    let employee_workflows = body_json(
        get(
            &app,
            &format!("/api/books/{book_id}/workflows/mine?entity_id={entity_id}"),
            &employee_cookie,
        )
        .await,
    )
    .await;
    assert_eq!(employee_workflows.as_array().unwrap().len(), 1);

    // Stranger: authenticated, but no role anywhere — empty, not an error.
    let stranger_books_resp = get(&app, "/api/books/mine", &stranger_cookie).await;
    assert_eq!(stranger_books_resp.status(), StatusCode::OK);
    assert_eq!(
        body_json(stranger_books_resp)
            .await
            .as_array()
            .unwrap()
            .len(),
        0
    );
    let stranger_workflows_resp = get(
        &app,
        &format!("/api/books/{book_id}/workflows/mine?entity_id={entity_id}"),
        &stranger_cookie,
    )
    .await;
    assert_eq!(stranger_workflows_resp.status(), StatusCode::OK);
    assert_eq!(
        body_json(stranger_workflows_resp)
            .await
            .as_array()
            .unwrap()
            .len(),
        0
    );
}

#[tokio::test]
async fn book_and_entity_picker_requires_authentication() {
    let books_dir = tempfile::tempdir().unwrap();
    let artifacts_dir = tempfile::tempdir().unwrap();
    let app = app_over(books_dir.path(), artifacts_dir.path());
    let response = call(&app, Method::GET, "/api/books/mine", None, None).await;
    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn l4_browser_generation_deployment_and_email_assignment_lifecycle() {
    let books_dir = tempfile::tempdir().unwrap();
    let artifacts_dir = tempfile::tempdir().unwrap();
    let frontend_dir = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(frontend_dir.path().join("workflow")).unwrap();
    std::fs::write(
        frontend_dir.path().join("workflow/workflow-react.js"),
        "export const React = {}; export function createRoot() {}",
    )
    .unwrap();
    let mut config = test_config(books_dir.path(), artifacts_dir.path());
    config.frontend_dist = frontend_dir.path().to_string_lossy().to_string();
    let app = build_router(Arc::new(AppState::new(config)));
    let (owner_cookie, _) = dev_login(&app, OWNER).await;
    let (book_id, entity_id) = create_book(&app, &owner_cookie).await;

    let generated = post(
        &app,
        &format!("/api/books/{book_id}/workflow-artifacts"),
        &owner_cookie,
        json!({
            "workflow_name": "L4 browser journal",
            "description": "Generated from the packaged browser template"
        }),
    )
    .await;
    assert_eq!(generated.status(), StatusCode::OK);
    let generated = body_json(generated).await;
    let deployment_id = generated["workflow_deployment_id"].as_str().unwrap();
    let workflow_id = generated["workflow_id"].as_str().unwrap();
    let artifact_dir = artifacts_dir
        .path()
        .join("persistent-generated/workflows")
        .join(deployment_id);
    assert!(artifact_dir.join("workflow.json").is_file());
    assert!(artifact_dir.join("manifest.json").is_file());
    assert!(artifact_dir.join("code/index.html").is_file());
    assert!(artifact_dir.join("code/app.js").is_file());
    assert!(artifact_dir.join("code/workflow-react.js").is_file());

    let artifacts = body_json(
        get(
            &app,
            &format!("/api/books/{book_id}/workflow-artifacts"),
            &owner_cookie,
        )
        .await,
    )
    .await;
    assert!(artifacts
        .as_array()
        .unwrap()
        .iter()
        .any(|artifact| { artifact["workflow_deployment_id"] == deployment_id }));

    let deployed = post(
        &app,
        &format!("/api/books/{book_id}/workflows/deploy"),
        &owner_cookie,
        json!({
            "workflow_deployment_id": deployment_id,
            "workflow_id": workflow_id,
            "entity_id": entity_id,
            "workflow_name": generated["workflow_name"],
            "description": generated["description"],
            "backend_api_calls": generated["backend_api_calls"],
            "required_inputs": generated["required_inputs"]
        }),
    )
    .await;
    assert_eq!(deployed.status(), StatusCode::OK);

    let standalone = call(
        &app,
        Method::GET,
        &format!("/workflows/{deployment_id}/code/index.html"),
        None,
        None,
    )
    .await;
    assert_eq!(standalone.status(), StatusCode::OK);
    let standalone = standalone.into_body().collect().await.unwrap().to_bytes();
    assert!(String::from_utf8_lossy(&standalone).contains("<title>FPA workflow</title>"));

    let roles = body_json(
        get(
            &app,
            &format!("/api/books/{book_id}/roles?entity_id={entity_id}"),
            &owner_cookie,
        )
        .await,
    )
    .await;
    let role_id = roles[0]["role_id"].as_str().unwrap();
    let assigned = post(
        &app,
        &format!("/api/books/{book_id}/roles/{role_id}/users"),
        &owner_cookie,
        json!({ "op_id": Uuid::new_v4(), "user_email": EMPLOYEE }),
    )
    .await;
    assert_eq!(assigned.status(), StatusCode::OK);

    let (employee_cookie, employee_id) = dev_login(&app, EMPLOYEE).await;
    let mine = body_json(
        get(
            &app,
            &format!("/api/books/{book_id}/workflows/mine?entity_id={entity_id}"),
            &employee_cookie,
        )
        .await,
    )
    .await;
    assert_eq!(mine.as_array().unwrap().len(), 1);
    let roles = body_json(
        get(
            &app,
            &format!("/api/books/{book_id}/roles?entity_id={entity_id}"),
            &owner_cookie,
        )
        .await,
    )
    .await;
    assert_eq!(roles[0]["assigned_user_ids"][0], employee_id.to_string());
    let replacement = body_json(
        post(
            &app,
            &format!("/api/books/{book_id}/workflow-artifacts"),
            &owner_cookie,
            json!({"workflow_name":"L4 browser journal","description":"Updated template"}),
        )
        .await,
    )
    .await;
    assert_eq!(replacement["workflow_id"], workflow_id);
    let replacement_id = replacement["workflow_deployment_id"].as_str().unwrap();
    assert_ne!(replacement_id, deployment_id);
    let replaced = post(&app, &format!("/api/books/{book_id}/workflows/deploy"), &owner_cookie,
        json!({"workflow_deployment_id":replacement_id,"workflow_id":workflow_id,"entity_id":entity_id,
            "workflow_name":"L4 browser journal","description":"Updated template",
            "backend_api_calls":replacement["backend_api_calls"],"required_inputs":replacement["required_inputs"],
            "metadata":replacement["metadata"]})).await;
    assert_eq!(replaced.status(), StatusCode::OK);
    let grant = post(
        &app,
        &format!("/api/books/{book_id}/roles/{role_id}/permissions"),
        &owner_cookie,
        json!({"op_id":Uuid::new_v4(),"permission":"list_resource_types"}),
    )
    .await;
    assert_eq!(grant.status(), StatusCode::OK);
    let mine = body_json(
        get(
            &app,
            &format!("/api/books/{book_id}/workflows/mine?entity_id={entity_id}"),
            &employee_cookie,
        )
        .await,
    )
    .await;
    assert_eq!(mine.as_array().unwrap().len(), 1);
    assert_eq!(mine[0]["workflow_deployment_id"], replacement_id);
    let roles = body_json(
        get(
            &app,
            &format!("/api/books/{book_id}/roles?entity_id={entity_id}"),
            &owner_cookie,
        )
        .await,
    )
    .await;
    assert_eq!(roles.as_array().unwrap().len(), 1);
    assert_eq!(roles[0]["assigned_user_ids"][0], employee_id.to_string());
    assert!(roles[0]["permissions"]
        .as_array()
        .unwrap()
        .contains(&json!("list_resource_types")));
}

#[tokio::test]
async fn opening_import_is_role_scoped_atomic_and_replay_safe() {
    let books_dir = tempfile::tempdir().unwrap();
    let artifacts_dir = tempfile::tempdir().unwrap();
    let app = app_over(books_dir.path(), artifacts_dir.path());
    let (owner_cookie, _) = dev_login(&app, OWNER).await;
    let (book_id, entity_id, cash, rent) = setup_book(&app, &owner_cookie).await;
    let charts = body_json(
        get(
            &app,
            &format!("/api/books/{book_id}/charts?entity_id={entity_id}"),
            &owner_cookie,
        )
        .await,
    )
    .await;
    let chart_id = charts[0]["chart_id"].as_str().unwrap();
    let deployment_id = Uuid::new_v4();
    let workflow_id = Uuid::new_v4();
    write_artifact(artifacts_dir.path(), deployment_id);
    id_field(
        post(
            &app,
            &format!("/api/books/{book_id}/workflows/deploy"),
            &owner_cookie,
            json!({
                "workflow_deployment_id": deployment_id, "workflow_id": workflow_id,
                "entity_id": entity_id, "workflow_name": "Opening import",
                "backend_api_calls": ["post_entry"], "metadata": {"kind":"opening_balance_import"}
            }),
        )
        .await,
    )
    .await;
    let roles = body_json(
        get(
            &app,
            &format!("/api/books/{book_id}/roles?entity_id={entity_id}"),
            &owner_cookie,
        )
        .await,
    )
    .await;
    let role_id = roles[0]["role_id"].as_str().unwrap();
    let package = json!({
        "schema_version":"1.0", "import_id":Uuid::new_v4().to_string(),
        "source_balance_date":"2026-02-09", "opening_entry_date":"2026-02-10",
        "declared_accounting_method":"cash", "declared_balance_basis":"Reviewed fixture",
        "resource_codes":["USD"],
        "source_material":[{"source_id":"fixture","sha256":"a".repeat(64),"label":"Test fixture"}],
        "balance_rows":[
            {"external_account_key":"cash","proposed_account_name":"Cash","proposed_account_type":"ASSET","resource_code":"USD","debit":"10.00","credit":"0","source_id":"fixture","source_reference":null,"property_reference":null,"note":null},
            {"external_account_key":"rent","proposed_account_name":"Rent Expense","proposed_account_type":"EXPENSE","resource_code":"USD","debit":"0","credit":"10.00","source_id":"fixture","source_reference":null,"property_reference":null,"note":null}
        ],
        "control_totals":{"USD":{"debit":"10.00","credit":"10.00"}}
    });
    let request = json!({"file_content":package.to_string(),"chart_id":chart_id,
        "account_mappings":{"cash":cash,"rent":rent},
        "workflow":{"workflow_id":workflow_id,"workflow_deployment_id":deployment_id,"workflow_execution_id":Uuid::new_v4()}});
    let path = format!("/api/books/{book_id}/opening-import");
    assert_eq!(
        post(&app, &path, &owner_cookie, request.clone())
            .await
            .status(),
        StatusCode::FORBIDDEN
    );
    id_field(
        post(
            &app,
            &format!("/api/books/{book_id}/roles/{role_id}/users"),
            &owner_cookie,
            json!({"op_id":Uuid::new_v4(),"assign_to_self":true}),
        )
        .await,
    )
    .await;
    assert_eq!(
        get(
            &app,
            &format!("{path}/context?workflow_deployment_id={deployment_id}"),
            &owner_cookie
        )
        .await
        .status(),
        StatusCode::OK
    );
    let bypass = post(&app, &format!("/api/books/{book_id}/entries"), &owner_cookie, json!({
        "entry_id":Uuid::new_v4(), "entity_id":entity_id, "entry_date":"2026-02-10",
        "description":"bypass", "source":"WORKFLOW",
        "workflow":{"workflow_id":workflow_id,"workflow_deployment_id":deployment_id,"workflow_execution_id":Uuid::new_v4()},
        "lines":[
            {"line_id":Uuid::new_v4(),"account_id":cash,"debit_amount":"10.00","credit_amount":null},
            {"line_id":Uuid::new_v4(),"account_id":rent,"debit_amount":null,"credit_amount":"10.00"}
        ]
    })).await;
    assert_eq!(bypass.status(), StatusCode::BAD_REQUEST);
    let first = id_field(post(&app, &path, &owner_cookie, request.clone()).await).await;
    assert_eq!(first.to_string(), package["import_id"].as_str().unwrap());
    let replay = post(&app, &path, &owner_cookie, request.clone()).await;
    assert_eq!(replay.status(), StatusCode::OK);
    assert_eq!(body_json(replay).await["id"], first.to_string());
    let mut bad_package = package.clone();
    bad_package["import_id"] = json!(Uuid::new_v4().to_string());
    bad_package["balance_rows"][1]["credit"] = json!("9.00");
    let mut bad_request = request.clone();
    bad_request["file_content"] = json!(bad_package.to_string());
    assert_eq!(
        post(&app, &path, &owner_cookie, bad_request).await.status(),
        StatusCode::BAD_REQUEST
    );
    let mut unmapped_package = package.clone();
    unmapped_package["import_id"] = json!(Uuid::new_v4().to_string());
    let mut unmapped_request = request.clone();
    unmapped_request["file_content"] = json!(unmapped_package.to_string());
    unmapped_request["account_mappings"]
        .as_object_mut()
        .unwrap()
        .remove("rent");
    assert_eq!(
        post(&app, &path, &owner_cookie, unmapped_request)
            .await
            .status(),
        StatusCode::BAD_REQUEST
    );
    let malformed_request = json!({"file_content":"{bad json", "chart_id":chart_id,
        "account_mappings":{"cash":cash,"rent":rent},
        "workflow":{"workflow_id":workflow_id,"workflow_deployment_id":deployment_id,"workflow_execution_id":Uuid::new_v4()}});
    assert_eq!(
        post(&app, &path, &owner_cookie, malformed_request)
            .await
            .status(),
        StatusCode::BAD_REQUEST
    );
    let mut closed_period_package = package.clone();
    closed_period_package["import_id"] = json!(Uuid::new_v4().to_string());
    closed_period_package["opening_entry_date"] = json!("2026-03-01");
    let mut closed_period_request = request;
    closed_period_request["file_content"] = json!(closed_period_package.to_string());
    assert_eq!(
        post(&app, &path, &owner_cookie, closed_period_request)
            .await
            .status(),
        StatusCode::UNPROCESSABLE_ENTITY
    );
    let entries = body_json(
        get(
            &app,
            &format!("/api/books/{book_id}/entries?entity_id={entity_id}"),
            &owner_cookie,
        )
        .await,
    )
    .await;
    assert_eq!(entries.as_array().unwrap().len(), 1);
}

#[tokio::test]
async fn v11_import_prepares_stable_properties_then_posts_attributed_entry() {
    let books_dir = tempfile::tempdir().unwrap();
    let artifacts_dir = tempfile::tempdir().unwrap();
    let app = app_over(books_dir.path(), artifacts_dir.path());
    let (owner_cookie, _) = dev_login(&app, OWNER).await;
    let (book_id, subject_id, _cash, rent) = setup_book(&app, &owner_cookie).await;
    let charts = body_json(
        get(
            &app,
            &format!("/api/books/{book_id}/charts?entity_id={subject_id}"),
            &owner_cookie,
        )
        .await,
    )
    .await;
    let chart_id = charts[0]["chart_id"].as_str().unwrap();
    let resources = body_json(
        get(
            &app,
            &format!("/api/books/{book_id}/resource-types"),
            &owner_cookie,
        )
        .await,
    )
    .await;
    let usd = resources[0]["resource_type_id"].as_str().unwrap();
    let deployment_id = Uuid::new_v4();
    let workflow_id = Uuid::new_v4();
    write_artifact(artifacts_dir.path(), deployment_id);
    id_field(post(&app, &format!("/api/books/{book_id}/workflows/deploy"), &owner_cookie,
        json!({"workflow_deployment_id":deployment_id,"workflow_id":workflow_id,"entity_id":subject_id,
            "workflow_name":"V11 import","backend_api_calls":["prepare_opening_import_entities","post_entry"],
            "metadata":{"kind":"opening_balance_import"}})).await).await;
    let roles = body_json(
        get(
            &app,
            &format!("/api/books/{book_id}/roles?entity_id={subject_id}"),
            &owner_cookie,
        )
        .await,
    )
    .await;
    let role_id = roles[0]["role_id"].as_str().unwrap();
    let (employee_cookie, employee_id) = dev_login(&app, EMPLOYEE).await;
    let package = json!({
        "schema_version":"1.1", "import_id":Uuid::new_v4().to_string(),
        "source_balance_date":"2026-02-09", "opening_entry_date":"2026-02-10",
        "declared_accounting_method":"cash", "declared_balance_basis":"Reviewed fixture",
        "resource_codes":["USD"], "entity_namespace":"zag-properties",
        "related_entities":[{"external_entity_key":"elm-house","name":"Elm House","category":"PROPERTY",
            "relationship":"OWNS","effective_from":"2026-01-01"}],
        "source_material":[{"source_id":"fixture","sha256":"a".repeat(64),"label":"Fixture"}],
        "balance_rows":[
            {"external_account_key":"property.asset","proposed_account_name":"Elm asset","proposed_account_type":"ASSET",
                "resource_code":"USD","debit":"10.00","credit":"0.00","source_id":"fixture",
                "source_reference":null,"property_reference":"Elm House","note":null,"attribution_entity_key":"elm-house"},
            {"external_account_key":"rent","proposed_account_name":"Rent Expense","proposed_account_type":"EXPENSE",
                "resource_code":"USD","debit":"0.00","credit":"10.00","source_id":"fixture",
                "source_reference":null,"property_reference":null,"note":null,"attribution_entity_key":null}
        ], "control_totals":{"USD":{"debit":"10.00","credit":"10.00"}}
    });
    let context = json!({"workflow_id":workflow_id,"workflow_deployment_id":deployment_id,"workflow_execution_id":Uuid::new_v4()});
    let prepare_path = format!("/api/books/{book_id}/opening-import/prepare-identities");
    let prep = json!({"file_content":package.to_string(),"chart_id":chart_id,"workflow":context});
    assert_eq!(
        post(&app, &prepare_path, &employee_cookie, prep.clone())
            .await
            .status(),
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        post(
            &app,
            &format!("/api/books/{book_id}/roles/{role_id}/users"),
            &owner_cookie,
            json!({"op_id":Uuid::new_v4(),"user_id":employee_id})
        )
        .await
        .status(),
        StatusCode::OK
    );
    assert_eq!(
        post(
            &app,
            &format!("/api/books/{book_id}/entities"),
            &employee_cookie,
            json!({"op_id":Uuid::new_v4(),"name":"No generic create","category":"PROPERTY"})
        )
        .await
        .status(),
        StatusCode::FORBIDDEN
    );
    let mut malformed = package.clone();
    malformed["control_totals"]["USD"]["debit"] = json!("11.00");
    assert_eq!(
        post(
            &app,
            &prepare_path,
            &employee_cookie,
            json!({"file_content":malformed.to_string(),"chart_id":chart_id,"workflow":context})
        )
        .await
        .status(),
        StatusCode::BAD_REQUEST
    );
    let before = body_json(
        get(
            &app,
            &format!("/api/books/{book_id}/entities"),
            &owner_cookie,
        )
        .await,
    )
    .await;
    assert_eq!(before.as_array().unwrap().len(), 1);
    let first = body_json(post(&app, &prepare_path, &employee_cookie, prep.clone()).await).await;
    let property_id = first["resolved_entities"][0]["entity_id"].as_str().unwrap();
    let second = body_json(post(&app, &prepare_path, &employee_cookie, prep).await).await;
    assert_eq!(second["resolved_entities"][0]["entity_id"], property_id);
    let mut revised = package.clone();
    revised["import_id"] = json!(Uuid::new_v4().to_string());
    let revised_prep =
        json!({"file_content":revised.to_string(),"chart_id":chart_id,"workflow":context});
    let reused = body_json(post(&app, &prepare_path, &employee_cookie, revised_prep).await).await;
    assert_eq!(reused["resolved_entities"][0]["entity_id"], property_id);
    let mut conflicting = package.clone();
    conflicting["import_id"] = json!(Uuid::new_v4().to_string());
    conflicting["related_entities"][0]["name"] = json!("Wrong identity");
    assert_eq!(
        post(
            &app,
            &prepare_path,
            &employee_cookie,
            json!({"file_content":conflicting.to_string(),"chart_id":chart_id,"workflow":context})
        )
        .await
        .status(),
        StatusCode::BAD_REQUEST
    );

    let import_path = format!("/api/books/{book_id}/opening-import");
    let missing = json!({"file_content":package.to_string(),"chart_id":chart_id,
        "account_mappings":{"property.asset":Uuid::new_v4(),"rent":rent},"workflow":context});
    assert_eq!(
        post(&app, &import_path, &employee_cookie, missing)
            .await
            .status(),
        StatusCode::BAD_REQUEST
    );
    let property_account = id_field(
        post(
            &app,
            &format!("/api/books/{book_id}/accounts"),
            &owner_cookie,
            json!({"op_id":Uuid::new_v4(),"chart_id":chart_id,"name":"Elm asset","code":null,
            "account_type":"ASSET","resource_type_id":usd,"parent_account_id":null,
            "associated_entity_id":property_id,"validation_rules":{},"metadata":{}}),
        )
        .await,
    )
    .await;
    let posting = json!({"file_content":package.to_string(),"chart_id":chart_id,
        "account_mappings":{"property.asset":property_account,"rent":rent},"workflow":context});
    let entry_id =
        id_field(post(&app, &import_path, &employee_cookie, posting.clone()).await).await;
    assert_eq!(
        id_field(post(&app, &import_path, &employee_cookie, posting).await).await,
        entry_id
    );
    let entries = body_json(
        get(
            &app,
            &format!("/api/books/{book_id}/entries?entity_id={subject_id}"),
            &owner_cookie,
        )
        .await,
    )
    .await;
    let entry = entries
        .as_array()
        .unwrap()
        .iter()
        .find(|entry| entry["entry_id"] == entry_id.to_string())
        .unwrap();
    assert_eq!(entry["lines"][0]["attribution_entity_id"], property_id);
}

#[tokio::test]
async fn zag_v11_draft_posts_to_a_disposable_book_when_fixture_path_is_set() {
    let Ok(path) =
        std::env::var("FPA_ZAG_V12_DRAFT").or_else(|_| std::env::var("FPA_ZAG_V11_DRAFT"))
    else {
        return;
    };
    let file_content = std::fs::read_to_string(path).unwrap();
    let package: Value = serde_json::from_str(&file_content).unwrap();
    let v12 = package["schema_version"] == "1.2";
    assert!(v12 || package["schema_version"] == "1.1");
    assert_eq!(package["related_entities"].as_array().unwrap().len(), 6);
    let books_dir = tempfile::tempdir().unwrap();
    let artifacts_dir = tempfile::tempdir().unwrap();
    let app = app_over(books_dir.path(), artifacts_dir.path());
    let (owner_cookie, _) = dev_login(&app, OWNER).await;
    let (book_id, subject_id) = create_book(&app, &owner_cookie).await;
    let usd = id_field(
        post(
            &app,
            &format!("/api/books/{book_id}/resource-types"),
            &owner_cookie,
            json!({"op_id":Uuid::new_v4(),"name":"US Dollar","kind":"CURRENCY","code":"USD",
            "unit_of_measure":"USD","precision":2}),
        )
        .await,
    )
    .await;
    let chart_id = id_field(
        post(
            &app,
            &format!("/api/books/{book_id}/charts"),
            &owner_cookie,
            json!({"op_id":Uuid::new_v4(),"entity_id":subject_id,"name":"Zag staging chart",
            "description":null,"activate":true}),
        )
        .await,
    )
    .await;
    assert_eq!(
        post(
            &app,
            &format!("/api/books/{book_id}/periods"),
            &owner_cookie,
            json!({"op_id":Uuid::new_v4(),"entity_id":subject_id,"name":"January 2025",
            "start_date":"2025-01-01","end_date":"2025-01-31"})
        )
        .await
        .status(),
        StatusCode::OK
    );
    let deployment_id = Uuid::new_v4();
    let workflow_id = Uuid::new_v4();
    write_artifact(artifacts_dir.path(), deployment_id);
    id_field(post(&app, &format!("/api/books/{book_id}/workflows/deploy"), &owner_cookie,
        json!({"workflow_deployment_id":deployment_id,"workflow_id":workflow_id,"entity_id":subject_id,
            "workflow_name":"Zag staging import","backend_api_calls":["prepare_opening_import_entities","post_entry"],
            "metadata":{"kind":"opening_balance_import"}})).await).await;
    let roles = body_json(
        get(
            &app,
            &format!("/api/books/{book_id}/roles?entity_id={subject_id}"),
            &owner_cookie,
        )
        .await,
    )
    .await;
    let role_id = roles[0]["role_id"].as_str().unwrap();
    let (operator_cookie, operator_id) = dev_login(&app, EMPLOYEE).await;
    assert_eq!(
        post(
            &app,
            &format!("/api/books/{book_id}/roles/{role_id}/users"),
            &owner_cookie,
            json!({"op_id":Uuid::new_v4(),"user_id":operator_id})
        )
        .await
        .status(),
        StatusCode::OK
    );
    let workflow = json!({"workflow_id":workflow_id,"workflow_deployment_id":deployment_id,"workflow_execution_id":Uuid::new_v4()});
    if v12 {
        for permission in [
            "create_fixed_asset",
            "list_fixed_assets",
            "list_accounts",
            "list_account_balances",
        ] {
            id_field(
                post(
                    &app,
                    &format!("/api/books/{book_id}/roles/{role_id}/permissions"),
                    &owner_cookie,
                    json!({"op_id":Uuid::new_v4(),"permission":permission}),
                )
                .await,
            )
            .await;
        }
    }
    let prepared_response = post(
        &app,
        &format!("/api/books/{book_id}/opening-import/prepare-identities"),
        &operator_cookie,
        json!({"file_content":file_content,"chart_id":chart_id,"workflow":workflow}),
    )
    .await;
    let prepared_status = prepared_response.status();
    let prepared = body_json(prepared_response).await;
    assert_eq!(prepared_status, StatusCode::OK, "{prepared}");
    let resolved = prepared["resolved_entities"].as_array().unwrap();
    assert_eq!(resolved.len(), 6);
    let by_key: std::collections::HashMap<_, _> = resolved
        .iter()
        .map(|item| {
            (
                item["external_entity_key"].as_str().unwrap(),
                item["entity_id"].as_str().unwrap(),
            )
        })
        .collect();
    let mut mappings = serde_json::Map::new();
    for row in package[if v12 {
        "account_definitions"
    } else {
        "balance_rows"
    }]
    .as_array()
    .unwrap()
    {
        let key = row["external_account_key"].as_str().unwrap();
        let association = row["attribution_entity_key"]
            .as_str()
            .map(|key| by_key[key]);
        let response = post(&app, &format!("/api/books/{book_id}/accounts"), &owner_cookie,
            json!({"op_id":Uuid::new_v4(),"chart_id":chart_id,
                "name":format!("{} [{key}]",row["proposed_account_name"].as_str().unwrap()),
                "code":null,"account_type":row["proposed_account_type"],"resource_type_id":usd,
                "parent_account_id":null,"associated_entity_id":association,"validation_rules":{},"metadata":{}})).await;
        let status = response.status();
        let body = body_json(response).await;
        assert_eq!(status, StatusCode::OK, "{body}");
        let account_id = body["id"].as_str().unwrap().to_owned();
        mappings.insert(key.to_owned(), json!(account_id));
    }
    let post_response = post(&app, &format!("/api/books/{book_id}/opening-import"), &operator_cookie,
        json!({"file_content":file_content,"chart_id":chart_id,"account_mappings":mappings,"workflow":workflow})).await;
    let post_status = post_response.status();
    let post_body = body_json(post_response).await;
    assert_eq!(post_status, StatusCode::OK, "{post_body}");
    let entry_id = post_body["id"].as_str().unwrap().to_owned();
    let entries = body_json(
        get(
            &app,
            &format!("/api/books/{book_id}/entries?entity_id={subject_id}"),
            &owner_cookie,
        )
        .await,
    )
    .await;
    let entry = entries
        .as_array()
        .unwrap()
        .iter()
        .find(|entry| entry["entry_id"] == entry_id)
        .unwrap();
    assert_eq!(entry["lines"].as_array().unwrap().len(), 21);
    assert_eq!(
        entry["lines"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|line| !line["attribution_entity_id"].is_null())
            .count(),
        18
    );
    let original_review = if v12 {
        let review = body_json(
            get(
                &app,
                &format!("/api/books/{book_id}/opening-import/review"),
                &operator_cookie,
            )
            .await,
        )
        .await;
        assert_eq!(review["assets"].as_array().unwrap().len(), 16);
        assert_eq!(review["property_profiles"].as_array().unwrap().len(), 6);
        assert_eq!(review["balances"].as_array().unwrap().len(), 93);
        let sum = |field: &str| {
            review["assets"].as_array().unwrap().iter().fold(
                ledgerzero_engine::amount::Amount::ZERO,
                |total, asset| {
                    total
                        .checked_add(asset[field].as_str().unwrap().parse().unwrap())
                        .unwrap()
                },
            )
        };
        for field in ["cost", "land", "accumulated_depreciation"] {
            let expected = package["fixed_assets"].as_array().unwrap().iter().fold(
                ledgerzero_engine::amount::Amount::ZERO,
                |total, asset| {
                    total
                        .checked_add(asset[field].as_str().unwrap().parse().unwrap())
                        .unwrap()
                },
            );
            assert_eq!(sum(field), expected);
        }
        Some(review)
    } else {
        None
    };
    let backup_location = tempfile::tempdir().unwrap();
    assert_eq!(
        post(
            &app,
            &format!("/api/books/{book_id}/backup"),
            &owner_cookie,
            json!({"location":backup_location.path().to_string_lossy()})
        )
        .await
        .status(),
        StatusCode::OK
    );
    assert_eq!(
        post(
            &app,
            &format!("/api/books/{book_id}/close"),
            &owner_cookie,
            json!({})
        )
        .await
        .status(),
        StatusCode::OK
    );
    assert_eq!(
        post(
            &app,
            &format!("/api/books/{book_id}/open"),
            &owner_cookie,
            json!({"passphrase":"correct horse battery staple"})
        )
        .await
        .status(),
        StatusCode::OK
    );
    let reopened = body_json(
        get(
            &app,
            &format!("/api/books/{book_id}/entries?entity_id={subject_id}"),
            &owner_cookie,
        )
        .await,
    )
    .await;
    assert!(reopened
        .as_array()
        .unwrap()
        .iter()
        .any(|entry| entry["entry_id"] == entry_id));
    if let Some(review) = original_review {
        assert_eq!(
            review,
            body_json(
                get(
                    &app,
                    &format!("/api/books/{book_id}/opening-import/review"),
                    &operator_cookie
                )
                .await
            )
            .await
        );
    }
}

#[tokio::test]
async fn opening_import_generator_packages_the_specific_spa_and_metadata() {
    let artifacts = tempfile::tempdir().unwrap();
    let frontend = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(frontend.path().join("workflow")).unwrap();
    std::fs::write(
        frontend.path().join("workflow/workflow-react.js"),
        "export const React = {};",
    )
    .unwrap();
    let generated = ledgerzero_backend::workflow_generation::generate(
        artifacts.path().to_str().unwrap(),
        frontend.path().to_str().unwrap(),
        ledgerzero_backend::workflow_generation::GenerateWorkflowArtifactRequest {
            workflow_name: "Opening import".into(),
            description: None,
            kind: Some("opening_balance_import".into()),
        },
    )
    .await
    .unwrap();
    assert_eq!(generated.metadata["kind"], "opening_balance_import");
    let app_js =
        std::fs::read_to_string(std::path::Path::new(&generated.artifact_path).join("code/app.js"))
            .unwrap();
    assert!(app_js.contains("/opening-import"));
    assert!(app_js.contains("Finish mapping"));
    assert!(app_js.contains("Create new"));
    assert!(app_js.contains("/accounts`"));
    assert!(app_js.contains("Post opening balances"));
    assert!(app_js.contains(&generated.workflow_id.to_string()));
}

#[tokio::test]
async fn v12_import_register_permissions_reconciliation_and_reopen() {
    let books = tempfile::tempdir().unwrap();
    let artifacts = tempfile::tempdir().unwrap();
    let app = app_over(books.path(), artifacts.path());
    let (owner, _) = dev_login(&app, OWNER).await;
    let (book, subject, _, _) = setup_book(&app, &owner).await;
    let charts = body_json(
        get(
            &app,
            &format!("/api/books/{book}/charts?entity_id={subject}"),
            &owner,
        )
        .await,
    )
    .await;
    let chart = charts[0]["chart_id"].clone();
    let resources =
        body_json(get(&app, &format!("/api/books/{book}/resource-types"), &owner).await).await;
    let resource = resources[0]["resource_type_id"].clone();
    let deployment = Uuid::new_v4();
    let workflow_id = Uuid::new_v4();
    write_artifact(artifacts.path(), deployment);
    id_field(post(&app, &format!("/api/books/{book}/workflows/deploy"), &owner,
        json!({"workflow_deployment_id":deployment,"workflow_id":workflow_id,"entity_id":subject,
            "workflow_name":"Register import","backend_api_calls":["prepare_opening_import_entities","post_entry"],
            "metadata":{"kind":"opening_balance_import"}})).await).await;
    let roles = body_json(
        get(
            &app,
            &format!("/api/books/{book}/roles?entity_id={subject}"),
            &owner,
        )
        .await,
    )
    .await;
    let role = roles[0]["role_id"].as_str().unwrap();
    let (operator, operator_id) = dev_login(&app, EMPLOYEE).await;
    id_field(
        post(
            &app,
            &format!("/api/books/{book}/roles/{role}/users"),
            &owner,
            json!({"op_id":Uuid::new_v4(),"user_id":operator_id}),
        )
        .await,
    )
    .await;
    let definitions = json!([
        {"external_account_key":"asset","proposed_account_name":"Oak cost","proposed_account_type":"ASSET","resource_code":"USD","attribution_entity_key":"oak","parent_external_account_key":null},
        {"external_account_key":"contra","proposed_account_name":"Oak accumulated depreciation","proposed_account_type":"ASSET","resource_code":"USD","attribution_entity_key":"oak","parent_external_account_key":null},
        {"external_account_key":"depreciation","proposed_account_name":"Oak depreciation","proposed_account_type":"EXPENSE","resource_code":"USD","attribution_entity_key":"oak","parent_external_account_key":null},
        {"external_account_key":"equity","proposed_account_name":"Opening equity","proposed_account_type":"EQUITY","resource_code":"USD","attribution_entity_key":null,"parent_external_account_key":null},
        {"external_account_key":"income","proposed_account_name":"Oak rent","proposed_account_type":"REVENUE","resource_code":"USD","attribution_entity_key":"oak","parent_external_account_key":null}
    ]);
    let mut package = json!({
        "schema_version":"1.2","import_id":Uuid::new_v4(),"source_balance_date":"2026-02-09","opening_entry_date":"2026-02-10",
        "declared_accounting_method":"cash","declared_balance_basis":"Reviewed example","resource_codes":["USD"],
        "entity_namespace":"example","related_entities":[{"external_entity_key":"oak","name":"Oak","category":"PROPERTY","relationship":"OWNS","effective_from":"2026-02-10"}],
        "source_material":[{"source_id":"fixture","sha256":"a".repeat(64),"label":"Example"}],
        "balance_rows":[
            {"external_account_key":"asset","proposed_account_name":"Oak cost","proposed_account_type":"ASSET","resource_code":"USD","debit":"10","credit":"0","source_id":"fixture","source_reference":"1","property_reference":"Oak","note":null,"attribution_entity_key":"oak"},
            {"external_account_key":"contra","proposed_account_name":"Oak accumulated depreciation","proposed_account_type":"ASSET","resource_code":"USD","debit":"0","credit":"2","source_id":"fixture","source_reference":"1","property_reference":"Oak","note":null,"attribution_entity_key":"oak"},
            {"external_account_key":"equity","proposed_account_name":"Opening equity","proposed_account_type":"EQUITY","resource_code":"USD","debit":"0","credit":"8","source_id":"fixture","source_reference":"1","property_reference":null,"note":null,"attribution_entity_key":null}],
        "control_totals":{"USD":{"debit":"10","credit":"10"}},"account_definitions":definitions,
        "property_facts":[{"external_entity_key":"oak","address":"1 Example Street","property_type":"MULTI_FAMILY_RESIDENCE","source_year":2026,"fair_rental_days":40,"personal_use_days":0,"source_id":"fixture","source_reference":"2"}],
        "fixed_assets":[{"external_asset_key":"oak.building","name":"Building","attribution_entity_key":"oak","cost_account_key":"asset","accumulated_depreciation_account_key":"contra","depreciation_expense_account_key":"depreciation","land_account_key":null,
            "in_service_date":"2020-01-01","cost":"10","land":"0","depreciable_basis":"10","business_use_percent":"100","useful_life_years":"27.50","method":"SL","convention":"MM",
            "accumulated_depreciation_as_of":"2026-02-09","accumulated_depreciation":"2","schedules":[{"year":2026,"amount":"1","status":"PROJECTED","basis":"TAX_EQUALS_BOOK"}],"source_id":"fixture","source_reference":"3"}]
    });
    let workflow = json!({"workflow_id":workflow_id,"workflow_deployment_id":deployment,"workflow_execution_id":Uuid::new_v4()});
    let prepare = |package: &Value| json!({"file_content":package.to_string(),"chart_id":chart,"workflow":workflow});
    let pre_import_backup = tempfile::tempdir().unwrap();
    assert_eq!(
        post(
            &app,
            &format!("/api/books/{book}/backup"),
            &owner,
            json!({"location":pre_import_backup.path()})
        )
        .await
        .status(),
        StatusCode::OK
    );
    let mut bad = package.clone();
    bad["fixed_assets"][0]["cost"] = json!("11");
    assert_eq!(
        post(
            &app,
            &format!("/api/books/{book}/opening-import/prepare-identities"),
            &operator,
            prepare(&bad)
        )
        .await
        .status(),
        StatusCode::BAD_REQUEST
    );
    let mut zero_depreciation = package.clone();
    zero_depreciation["import_id"] = json!(Uuid::new_v4());
    zero_depreciation["fixed_assets"][0]["accumulated_depreciation"] = json!("0");
    zero_depreciation["balance_rows"]
        .as_array_mut()
        .unwrap()
        .remove(1);
    zero_depreciation["balance_rows"][1]["credit"] = json!("10");
    assert_eq!(
        post(
            &app,
            &format!("/api/books/{book}/opening-import/prepare-identities"),
            &operator,
            prepare(&zero_depreciation)
        )
        .await
        .status(),
        StatusCode::OK
    );
    let prepared = body_json(
        post(
            &app,
            &format!("/api/books/{book}/opening-import/prepare-identities"),
            &operator,
            prepare(&package),
        )
        .await,
    )
    .await;
    let property = prepared["resolved_entities"][0]["entity_id"].clone();
    assert!(property.is_string(), "{prepared}");
    let mut mappings = serde_json::Map::new();
    for definition in definitions.as_array().unwrap() {
        let id = id_field(post(&app, &format!("/api/books/{book}/accounts"), &owner,
            json!({"op_id":Uuid::new_v4(),"chart_id":chart,"name":definition["proposed_account_name"],"account_type":definition["proposed_account_type"],
                "resource_type_id":resource,"parent_account_id":null,"associated_entity_id":if definition["attribution_entity_key"].is_null(){Value::Null}else{property.clone()},"validation_rules":{},"metadata":{}})).await).await;
        mappings.insert(
            definition["external_account_key"].as_str().unwrap().into(),
            json!(id),
        );
    }
    let request = |package: &Value| json!({"file_content":package.to_string(),"chart_id":chart,"account_mappings":mappings,"workflow":workflow});
    let path = format!("/api/books/{book}/opening-import");
    assert_eq!(
        post(&app, &path, &operator, request(&package))
            .await
            .status(),
        StatusCode::FORBIDDEN
    );
    id_field(
        post(
            &app,
            &format!("/api/books/{book}/roles/{role}/permissions"),
            &owner,
            json!({"op_id":Uuid::new_v4(),"permission":"create_fixed_asset"}),
        )
        .await,
    )
    .await;
    let first = post(&app, &path, &operator, request(&package)).await;
    let status = first.status();
    let result = body_json(first).await;
    assert_eq!(status, StatusCode::OK, "{result}");
    assert_eq!(
        post(&app, &path, &operator, request(&package))
            .await
            .status(),
        StatusCode::OK
    );
    assert_eq!(
        get(&app, &format!("/api/books/{book}/fixed-assets"), &operator)
            .await
            .status(),
        StatusCode::FORBIDDEN
    );
    id_field(
        post(
            &app,
            &format!("/api/books/{book}/roles/{role}/permissions"),
            &owner,
            json!({"op_id":Uuid::new_v4(),"permission":"list_fixed_assets"}),
        )
        .await,
    )
    .await;
    assert_eq!(
        get(
            &app,
            &format!("/api/books/{book}/opening-import/review"),
            &operator
        )
        .await
        .status(),
        StatusCode::FORBIDDEN
    );
    for permission in ["list_accounts", "list_account_balances"] {
        id_field(
            post(
                &app,
                &format!("/api/books/{book}/roles/{role}/permissions"),
                &owner,
                json!({"op_id":Uuid::new_v4(),"permission":permission}),
            )
            .await,
        )
        .await;
    }
    let mut review = body_json(
        get(
            &app,
            &format!("/api/books/{book}/opening-import/review"),
            &operator,
        )
        .await,
    )
    .await;
    assert_eq!(review["assets"].as_array().unwrap().len(), 1);
    assert_eq!(
        review["property_profiles"][0]["address"],
        "1 Example Street"
    );
    assert_eq!(review["assets"][0]["schedules"][0]["amount"], "1.00000000");
    let entries = body_json(
        get(
            &app,
            &format!("/api/books/{book}/entries?entity_id={subject}"),
            &owner,
        )
        .await,
    )
    .await;
    assert_eq!(entries.as_array().unwrap().len(), 1);
    assert_eq!(entries[0]["lines"].as_array().unwrap().len(), 3);
    package["import_id"] = json!(Uuid::new_v4());
    package["fixed_assets"][0]["accumulated_depreciation"] = json!("3");
    assert_eq!(
        post(&app, &path, &operator, request(&package))
            .await
            .status(),
        StatusCode::BAD_REQUEST
    );
    assert_eq!(
        body_json(
            get(
                &app,
                &format!("/api/books/{book}/entries?entity_id={subject}"),
                &owner
            )
            .await
        )
        .await
        .as_array()
        .unwrap()
        .len(),
        1
    );
    let journal_deployment = Uuid::new_v4();
    let journal_workflow = Uuid::new_v4();
    write_artifact(artifacts.path(), journal_deployment);
    id_field(post(&app, &format!("/api/books/{book}/workflows/deploy"), &owner,
        json!({"workflow_deployment_id":journal_deployment,"workflow_id":journal_workflow,"entity_id":subject,
            "workflow_name":"Asset changes","backend_api_calls":["post_entry"],"metadata":{}})).await).await;
    let all_roles = body_json(
        get(
            &app,
            &format!("/api/books/{book}/roles?entity_id={subject}"),
            &owner,
        )
        .await,
    )
    .await;
    let journal_role = all_roles
        .as_array()
        .unwrap()
        .iter()
        .find(|r| r["name"] == "Asset changes")
        .unwrap()["role_id"]
        .as_str()
        .unwrap();
    id_field(
        post(
            &app,
            &format!("/api/books/{book}/roles/{journal_role}/users"),
            &owner,
            json!({"op_id":Uuid::new_v4(),"user_id":operator_id}),
        )
        .await,
    )
    .await;
    let entry_id = Uuid::new_v4();
    let journal_context = json!({"workflow_id":journal_workflow,"workflow_deployment_id":journal_deployment,"workflow_execution_id":Uuid::new_v4()});
    let mut asset = review["assets"][0].clone();
    asset["asset_id"] = json!(Uuid::new_v4());
    asset["external_key"] = json!("oak.roof");
    asset["name"] = json!("New roof");
    asset["cost"] = json!("3");
    asset["depreciable_basis"] = json!("3");
    asset["accumulated_depreciation"] = json!("0");
    asset["in_service_date"] = json!("2026-02-10");
    asset["accumulated_depreciation_as_of"] = json!("2026-02-10");
    asset["linked_entry_id"] = json!(entry_id);
    asset["schedules"][0]["amount"] = json!("0.10");
    let asset_entry = |entry_id: Uuid, amount: &str| {
        json!({"entry_id":entry_id,"entity_id":subject,
        "entry_date":"2026-02-10","description":"Capital improvement","source":"WORKFLOW","workflow":journal_context,
        "metadata":{},"lines":[{"line_id":Uuid::new_v4(),"account_id":mappings["asset"],
            "attribution_entity_id":property,"debit_amount":amount,"memo":null},
            {"line_id":Uuid::new_v4(),"account_id":mappings["equity"],"credit_amount":amount,"memo":null}]})
    };
    let asset_path = format!("/api/books/{book}/fixed-assets/transactions");
    let acquisition = json!({"entry":asset_entry(entry_id,"3"),"asset":asset});
    let acquired = post(&app, &asset_path, &operator, acquisition.clone()).await;
    let status = acquired.status();
    let value = body_json(acquired).await;
    assert_eq!(status, StatusCode::OK, "{value}");
    assert_eq!(
        post(&app, &asset_path, &operator, acquisition)
            .await
            .status(),
        StatusCode::OK
    );
    let mut improved = review["assets"][0].clone();
    let improvement_id = Uuid::new_v4();
    improved["linked_entry_id"] = json!(improvement_id);
    improved["cost"] = json!("12");
    improved["depreciable_basis"] = json!("12");
    let improvement = json!({"entry":asset_entry(improvement_id,"2"),"asset":improved});
    assert_eq!(
        post(&app, &asset_path, &operator, improvement.clone())
            .await
            .status(),
        StatusCode::FORBIDDEN
    );
    id_field(
        post(
            &app,
            &format!("/api/books/{book}/roles/{role}/permissions"),
            &owner,
            json!({"op_id":Uuid::new_v4(),"permission":"update_fixed_asset"}),
        )
        .await,
    )
    .await;
    let response = post(&app, &asset_path, &operator, improvement).await;
    let status = response.status();
    let value = body_json(response).await;
    assert_eq!(status, StatusCode::OK, "{value}");
    review = body_json(
        get(
            &app,
            &format!("/api/books/{book}/opening-import/review"),
            &operator,
        )
        .await,
    )
    .await;
    assert_eq!(review["assets"].as_array().unwrap().len(), 2);
    assert!(review["assets"]
        .as_array()
        .unwrap()
        .iter()
        .any(|a| a["cost"] == "12.00000000" && a["revision"] == 1));
    let backup = tempfile::tempdir().unwrap();
    assert_eq!(
        post(
            &app,
            &format!("/api/books/{book}/backup"),
            &owner,
            json!({"location":backup.path()})
        )
        .await
        .status(),
        StatusCode::OK
    );
    assert_eq!(
        post(&app, &format!("/api/books/{book}/close"), &owner, json!({}))
            .await
            .status(),
        StatusCode::OK
    );
    assert_eq!(
        post(
            &app,
            "/api/books/restore",
            &owner,
            json!({"location":backup.path()})
        )
        .await
        .status(),
        StatusCode::OK
    );
    assert_eq!(
        post(
            &app,
            &format!("/api/books/{book}/open"),
            &owner,
            json!({"passphrase":"correct horse battery staple"})
        )
        .await
        .status(),
        StatusCode::OK
    );
    let reopened = body_json(
        get(
            &app,
            &format!("/api/books/{book}/opening-import/review"),
            &operator,
        )
        .await,
    )
    .await;
    assert_eq!(review, reopened);
    assert_eq!(
        post(&app, &format!("/api/books/{book}/close"), &owner, json!({}))
            .await
            .status(),
        StatusCode::OK
    );
    assert_eq!(
        post(
            &app,
            "/api/books/restore",
            &owner,
            json!({"location":pre_import_backup.path()})
        )
        .await
        .status(),
        StatusCode::OK
    );
    assert_eq!(
        post(
            &app,
            &format!("/api/books/{book}/open"),
            &owner,
            json!({"passphrase":"correct horse battery staple"})
        )
        .await
        .status(),
        StatusCode::OK
    );
    let reset = body_json(
        get(
            &app,
            &format!("/api/books/{book}/opening-import/review"),
            &owner,
        )
        .await,
    )
    .await;
    assert!(reset["assets"].as_array().unwrap().is_empty());
    assert!(reset["property_profiles"].as_array().unwrap().is_empty());
    assert_eq!(reset["balances"].as_array().unwrap().len(), 2);
    assert!(body_json(
        get(
            &app,
            &format!("/api/books/{book}/entries?entity_id={subject}"),
            &owner
        )
        .await
    )
    .await
    .as_array()
    .unwrap()
    .is_empty());
}
