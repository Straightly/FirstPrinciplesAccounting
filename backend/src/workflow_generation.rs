//! Packaged deterministic workflow-artifact preparation for the L4 browser
//! administration surface. This module does not read accounting storage or
//! deploy anything. It turns a small, inspectable template request into the
//! same immutable standalone-SPA artifact shape used by the Python dev-time
//! generator; the existing deployment endpoint remains the registration and
//! integrity boundary.

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::path::{Path, PathBuf};
use uuid::Uuid;

#[derive(Debug, Deserialize)]
pub struct GenerateWorkflowArtifactRequest {
    pub workflow_name: String,
    pub description: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct WorkflowArtifactSummary {
    pub workflow_deployment_id: Uuid,
    pub workflow_id: Uuid,
    pub workflow_name: String,
    pub description: Option<String>,
    pub backend_api_calls: Vec<String>,
    pub required_inputs: Value,
    pub artifact_path: String,
}

fn workflows_root(dev_artifacts_dir: &str) -> PathBuf {
    Path::new(dev_artifacts_dir).join("workflows")
}

fn validate_request(request: &GenerateWorkflowArtifactRequest) -> Result<(), String> {
    let name = request.workflow_name.trim();
    if name.is_empty() {
        return Err("workflow name must not be empty".to_string());
    }
    if name.len() > 120 {
        return Err("workflow name must be 120 characters or fewer".to_string());
    }
    if request.description.as_deref().unwrap_or("").len() > 1000 {
        return Err("workflow description must be 1000 characters or fewer".to_string());
    }
    Ok(())
}

pub async fn generate(
    dev_artifacts_dir: &str,
    frontend_dist: &str,
    request: GenerateWorkflowArtifactRequest,
) -> Result<WorkflowArtifactSummary, String> {
    validate_request(&request)?;
    let workflow_deployment_id = Uuid::new_v4();
    let workflow_id = Uuid::new_v4();
    let root = workflows_root(dev_artifacts_dir);
    let target = root.join(workflow_deployment_id.to_string());
    let temporary = root.join(format!(".{workflow_deployment_id}.tmp"));
    let code_dir = temporary.join("code");
    let signatures_dir = temporary.join("signatures");
    let workflow_name = request.workflow_name.trim().to_string();
    let description = request
        .description
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty());
    let required_inputs = json!({
        "entry_date": "date",
        "description": "text",
        "amount": "number",
        "direction": "select",
        "primary_account_id": "account",
        "offset_account_id": "account",
        "memo": "text"
    });
    let workflow_json = json!({
        "workflow_name": workflow_name,
        "description": description,
        "steps": [
            {"kind": "form", "collects": ["Entry date", "Description", "Amount", "Direction", "Primary account", "Offset account", "Memo (optional)"]},
            {"kind": "api_call", "backend_api": "post_entry"}
        ],
        "backend_api_calls": ["post_entry"],
        "required_inputs": required_inputs
    });
    let manifest_json = json!({
        "workflow_deployment_id": workflow_deployment_id,
        "workflow_id": workflow_id,
        "generator": "packaged-template:v1",
        "generated_by": "ledgerzero-backend workflow artifact preparer",
        "code_files": ["index.html", "app.js", "workflow-react.js"]
    });
    let app_js = include_str!("workflow_template.js")
        .replace("__WORKFLOW_ID__", &workflow_id.to_string())
        .replace(
            "__WORKFLOW_DEPLOYMENT_ID__",
            &workflow_deployment_id.to_string(),
        )
        .replace(
            "__WORKFLOW_NAME_JSON__",
            &serde_json::to_string(&workflow_name).map_err(|error| error.to_string())?,
        );
    let vendor = Path::new(frontend_dist)
        .join("workflow")
        .join("workflow-react.js");
    if !vendor.is_file() {
        return Err(format!(
            "workflow React bundle is missing at {}; deploy a complete launcher artifact first",
            vendor.display()
        ));
    }

    tokio::fs::create_dir_all(&root)
        .await
        .map_err(|error| error.to_string())?;
    if target.exists() || temporary.exists() {
        return Err("generated workflow artifact id already exists".to_string());
    }
    let result: Result<(), String> = async {
        tokio::fs::create_dir_all(&code_dir)
            .await
            .map_err(|error| error.to_string())?;
        tokio::fs::create_dir_all(&signatures_dir)
            .await
            .map_err(|error| error.to_string())?;
        tokio::fs::write(
            temporary.join("workflow.json"),
            serde_json::to_vec_pretty(&workflow_json).map_err(|error| error.to_string())?,
        )
        .await
        .map_err(|error| error.to_string())?;
        tokio::fs::write(
            temporary.join("manifest.json"),
            serde_json::to_vec_pretty(&manifest_json).map_err(|error| error.to_string())?,
        )
        .await
        .map_err(|error| error.to_string())?;
        tokio::fs::write(
            code_dir.join("index.html"),
            include_str!("workflow_template.html"),
        )
        .await
        .map_err(|error| error.to_string())?;
        tokio::fs::write(code_dir.join("app.js"), app_js)
            .await
            .map_err(|error| error.to_string())?;
        tokio::fs::copy(&vendor, code_dir.join("workflow-react.js"))
            .await
            .map_err(|error| error.to_string())?;
        tokio::fs::write(signatures_dir.join(".gitkeep"), b"")
            .await
            .map_err(|error| error.to_string())?;
        tokio::fs::rename(&temporary, &target)
            .await
            .map_err(|error| error.to_string())?;
        Ok(())
    }
    .await;
    if let Err(error) = result {
        let _ = tokio::fs::remove_dir_all(&temporary).await;
        return Err(format!("failed to prepare workflow artifact: {error}"));
    }

    Ok(WorkflowArtifactSummary {
        workflow_deployment_id,
        workflow_id,
        workflow_name,
        description,
        backend_api_calls: vec!["post_entry".to_string()],
        required_inputs,
        artifact_path: target.to_string_lossy().into_owned(),
    })
}

pub async fn list(dev_artifacts_dir: &str) -> Result<Vec<WorkflowArtifactSummary>, String> {
    let root = workflows_root(dev_artifacts_dir);
    let mut entries = match tokio::fs::read_dir(&root).await {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(error) => return Err(error.to_string()),
    };
    let mut artifacts = Vec::new();
    while let Some(entry) = entries
        .next_entry()
        .await
        .map_err(|error| error.to_string())?
    {
        if !entry
            .file_type()
            .await
            .map_err(|error| error.to_string())?
            .is_dir()
        {
            continue;
        }
        let path = entry.path();
        let Ok(workflow_deployment_id) = entry.file_name().to_string_lossy().parse::<Uuid>() else {
            continue;
        };
        let workflow: Value = match tokio::fs::read(path.join("workflow.json")).await {
            Ok(bytes) => serde_json::from_slice(&bytes).map_err(|error| error.to_string())?,
            Err(_) => continue,
        };
        let manifest: Value = match tokio::fs::read(path.join("manifest.json")).await {
            Ok(bytes) => serde_json::from_slice(&bytes).map_err(|error| error.to_string())?,
            Err(_) => continue,
        };
        let workflow_id = manifest
            .get("workflow_id")
            .and_then(Value::as_str)
            .and_then(|value| value.parse::<Uuid>().ok())
            .ok_or_else(|| format!("artifact {workflow_deployment_id} has no valid workflow_id"))?;
        artifacts.push(WorkflowArtifactSummary {
            workflow_deployment_id,
            workflow_id,
            workflow_name: workflow
                .get("workflow_name")
                .and_then(Value::as_str)
                .unwrap_or("Unnamed workflow")
                .to_string(),
            description: workflow
                .get("description")
                .and_then(Value::as_str)
                .map(str::to_string),
            backend_api_calls: workflow
                .get("backend_api_calls")
                .and_then(Value::as_array)
                .map(|values| {
                    values
                        .iter()
                        .filter_map(Value::as_str)
                        .map(str::to_string)
                        .collect()
                })
                .unwrap_or_default(),
            required_inputs: workflow
                .get("required_inputs")
                .cloned()
                .unwrap_or_else(|| json!({})),
            artifact_path: path.to_string_lossy().into_owned(),
        });
    }
    artifacts.sort_by(|left, right| left.workflow_name.cmp(&right.workflow_name));
    Ok(artifacts)
}
