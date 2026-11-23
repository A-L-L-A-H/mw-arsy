use axum::{
    extract::{Path, Query, State},
    http::StatusCode,
    routing::{delete, get, post},
    Json, Router,
};
use serde::Deserialize;
use std::path::Path as StdPath;

use super::AppState;
use crate::safe_cmd::safe_command;
use crate::services::git_repos::{
    self, MergeFailure, GIT_REPOS_ROOT,
};

type ApiErr = (StatusCode, Json<serde_json::Value>);

fn err(status: StatusCode, msg: &str) -> ApiErr {
    (status, Json(serde_json::json!({ "error": msg })))
}

#[derive(Debug, Deserialize)]
pub struct InitRepoBody {
    #[serde(default = "default_branch")]
    pub default_branch: String,
}

fn default_branch() -> String {
    "main".to_string()
}

#[derive(Debug, Deserialize)]
pub struct CommitsQuery {
    #[serde(rename = "ref", default = "default_head")]
    pub git_ref: String,
    #[serde(default = "default_limit")]
    pub limit: usize,
    #[serde(default)]
    pub skip: usize,
}

fn default_head() -> String {
    "HEAD".to_string()
}

fn default_limit() -> usize {
    30
}

#[derive(Debug, Deserialize)]
pub struct TreeQuery {
    #[serde(rename = "ref", default = "default_head")]
    pub git_ref: String,
    #[serde(default)]
    pub path: String,
}

#[derive(Debug, Deserialize)]
pub struct DiffQuery {
    pub base: String,
    pub head: String,
}

#[derive(Debug, Deserialize)]
pub struct MergeBody {
    pub source: String,
    pub target: String,
    #[serde(default = "default_squash")]
    pub method: String,
    pub commit_message: String,
    pub author_name: String,
    pub author_email: String,
}

fn default_squash() -> String {
    "squash".to_string()
}

#[derive(Debug, Deserialize)]
pub struct CloneNativeBody {
    pub deploy_name: String,
    pub owner: String,
    pub slug: String,
    pub branch: String,
}

/// POST /repos/{owner}/{slug} — Create bare repository on disk.
pub async fn create_repo(
    State(_state): State<AppState>,
    Path((owner, slug)): Path<(String, String)>,
    Json(body): Json<InitRepoBody>,
) -> Result<(StatusCode, Json<serde_json::Value>), ApiErr> {
    let path = git_repos::init_repo(
        StdPath::new(GIT_REPOS_ROOT),
        &owner,
        &slug,
        &body.default_branch,
    )
    .await
    .map_err(|e| err(StatusCode::BAD_REQUEST, &e))?;

    Ok((
        StatusCode::CREATED,
        Json(serde_json::json!({
            "ok": true,
            "path": path.to_string_lossy(),
        })),
    ))
}

/// DELETE /repos/{owner}/{slug} — Remove bare repository from disk.
pub async fn remove_repo(
    State(_state): State<AppState>,
    Path((owner, slug)): Path<(String, String)>,
) -> Result<Json<serde_json::Value>, ApiErr> {
    git_repos::delete_repo(StdPath::new(GIT_REPOS_ROOT), &owner, &slug)
        .await
        .map_err(|e| err(StatusCode::BAD_REQUEST, &e))?;
    Ok(Json(serde_json::json!({ "ok": true })))
}

/// GET /repos/{owner}/{slug}/stats
pub async fn stats(
    State(_state): State<AppState>,
    Path((owner, slug)): Path<(String, String)>,
) -> Result<Json<git_repos::RepoStats>, ApiErr> {
    let s = git_repos::repo_stats(StdPath::new(GIT_REPOS_ROOT), &owner, &slug)
        .await
        .map_err(|e| err(StatusCode::NOT_FOUND, &e))?;
    Ok(Json(s))
}

/// GET /repos/{owner}/{slug}/branches
pub async fn branches(
    State(_state): State<AppState>,
    Path((owner, slug)): Path<(String, String)>,
) -> Result<Json<Vec<git_repos::BranchInfo>>, ApiErr> {
    let list = git_repos::list_branches(StdPath::new(GIT_REPOS_ROOT), &owner, &slug)
        .await
        .map_err(|e| err(StatusCode::NOT_FOUND, &e))?;
    Ok(Json(list))
}

/// GET /repos/{owner}/{slug}/commits
pub async fn commits(
    State(_state): State<AppState>,
    Path((owner, slug)): Path<(String, String)>,
    Query(q): Query<CommitsQuery>,
) -> Result<Json<Vec<git_repos::CommitSummary>>, ApiErr> {
    let list = git_repos::list_commits(
        StdPath::new(GIT_REPOS_ROOT),
        &owner,
        &slug,
        &q.git_ref,
        q.limit,
        q.skip,
    )
    .await
    .map_err(|e| err(StatusCode::BAD_REQUEST, &e))?;
    Ok(Json(list))
}

/// GET /repos/{owner}/{slug}/commit/{sha}
pub async fn commit_detail(
    State(_state): State<AppState>,
    Path((owner, slug, sha)): Path<(String, String, String)>,
) -> Result<Json<git_repos::CommitDetail>, ApiErr> {
    let detail = git_repos::commit_detail(StdPath::new(GIT_REPOS_ROOT), &owner, &slug, &sha)
        .await
        .map_err(|e| err(StatusCode::NOT_FOUND, &e))?;
    Ok(Json(detail))
}

/// GET /repos/{owner}/{slug}/tree
pub async fn tree(
    State(_state): State<AppState>,
    Path((owner, slug)): Path<(String, String)>,
    Query(q): Query<TreeQuery>,
) -> Result<Json<Vec<git_repos::TreeEntry>>, ApiErr> {
    let entries = git_repos::list_tree(
        StdPath::new(GIT_REPOS_ROOT),
        &owner,
        &slug,
        &q.git_ref,
        &q.path,
    )
    .await
    .map_err(|e| err(StatusCode::BAD_REQUEST, &e))?;
    Ok(Json(entries))
}

/// GET /repos/{owner}/{slug}/raw
pub async fn raw(
    State(_state): State<AppState>,
    Path((owner, slug)): Path<(String, String)>,
    Query(q): Query<TreeQuery>,
) -> Result<Json<serde_json::Value>, ApiErr> {
    let (bytes, truncated) = git_repos::raw_blob(
        StdPath::new(GIT_REPOS_ROOT),
        &owner,
        &slug,
        &q.git_ref,
        &q.path,
    )
    .await
    .map_err(|e| err(StatusCode::NOT_FOUND, &e))?;

    let is_binary = bytes.iter().take(8000).any(|&b| b == 0);
    let content = if is_binary {
        None
    } else {
        Some(String::from_utf8_lossy(&bytes).into_owned())
    };
    Ok(Json(serde_json::json!({
        "is_binary": is_binary,
        "truncated": truncated,
        "size": bytes.len(),
        "content": content,
    })))
}

/// GET /repos/{owner}/{slug}/diff?base=...&head=...
pub async fn diff(
    State(_state): State<AppState>,
    Path((owner, slug)): Path<(String, String)>,
    Query(q): Query<DiffQuery>,
) -> Result<Json<git_repos::DiffResult>, ApiErr> {
    let res = git_repos::diff_refs(
        StdPath::new(GIT_REPOS_ROOT),
        &owner,
        &slug,
        &q.base,
        &q.head,
    )
    .await
    .map_err(|e| err(StatusCode::BAD_REQUEST, &e))?;
    Ok(Json(res))
}

/// POST /repos/{owner}/{slug}/merge — Bare-safe FF or squash merge.
pub async fn merge(
    State(_state): State<AppState>,
    Path((owner, slug)): Path<(String, String)>,
    Json(body): Json<MergeBody>,
) -> Result<Json<git_repos::MergeResult>, ApiErr> {
    match git_repos::merge_refs(
        StdPath::new(GIT_REPOS_ROOT),
        &owner,
        &slug,
        &body.source,
        &body.target,
        &body.method,
        &body.commit_message,
        &body.author_name,
        &body.author_email,
    )
    .await
    {
        Ok(res) => Ok(Json(res)),
        Err(MergeFailure::Conflict(files)) => Err((
            StatusCode::CONFLICT,
            Json(serde_json::json!({
                "error": "Merge conflict",
                "conflicts": files,
            })),
        )),
        Err(MergeFailure::Invalid(msg)) => Err(err(StatusCode::UNPROCESSABLE_ENTITY, &msg)),
    }
}

/// POST /repos/clone-native — Clone/update a native built-in repository into
/// `/var/lib/arsy/git/{deploy_name}` for Git Deploy without SSRF loopback failure.
pub async fn clone_native(
    State(_state): State<AppState>,
    Json(body): Json<CloneNativeBody>,
) -> Result<Json<serde_json::Value>, ApiErr> {
    if !git_repos::is_valid_segment(&body.deploy_name)
        || !git_repos::is_valid_ref(&body.branch)
    {
        return Err(err(StatusCode::BAD_REQUEST, "Invalid deploy name or branch"));
    }
    let bare_path = git_repos::repo_path(StdPath::new(GIT_REPOS_ROOT), &body.owner, &body.slug)
        .map_err(|e| err(StatusCode::BAD_REQUEST, &e))?;
    if !bare_path.exists() {
        return Err(err(StatusCode::NOT_FOUND, "Native repository not found on agent"));
    }

    let checkout_dir = format!("/var/lib/arsy/git/{}", body.deploy_name);
    let _guard = crate::site_lock::lock_site(&format!("git-deploy:{}", body.deploy_name)).await;
    if StdPath::new(&checkout_dir).exists() {
        std::fs::remove_dir_all(&checkout_dir)
            .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, &format!("Cleanup failed: {e}")))?;
    }
    std::fs::create_dir_all(&checkout_dir)
        .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, &format!("Mkdir failed: {e}")))?;

    let bare_str = bare_path.to_string_lossy().to_string();
    let mut cmd = safe_command("git");
    cmd.args([
        "clone",
        "--branch",
        &body.branch,
        "--single-branch",
        "--depth",
        "50",
        &bare_str,
        &checkout_dir,
    ]);
    cmd.kill_on_drop(true);
    let out = tokio::time::timeout(std::time::Duration::from_secs(120), cmd.output())
        .await
        .map_err(|_| err(StatusCode::GATEWAY_TIMEOUT, "Native clone timed out"))?
        .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, &format!("git clone failed: {e}")))?;

    if !out.status.success() {
        std::fs::remove_dir_all(&checkout_dir).ok();
        return Err(err(
            StatusCode::BAD_REQUEST,
            &format!("git clone failed: {}", String::from_utf8_lossy(&out.stderr)),
        ));
    }

    let hash_out = safe_command("git")
        .args(["-C", &checkout_dir, "rev-parse", "--short", "HEAD"])
        .output()
        .await
        .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string()))?;
    let msg_out = safe_command("git")
        .args(["-C", &checkout_dir, "log", "-1", "--format=%s"])
        .output()
        .await
        .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string()))?;

    let commit_hash = String::from_utf8_lossy(&hash_out.stdout).trim().to_string();
    let commit_message = String::from_utf8_lossy(&msg_out.stdout).trim().to_string();

    Ok(Json(serde_json::json!({
        "commit_hash": commit_hash,
        "commit_message": commit_message,
    })))
}

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/repos/clone-native", post(clone_native))
        .route("/repos/{owner}/{slug}", post(create_repo).delete(remove_repo))
        .route("/repos/{owner}/{slug}/stats", get(stats))
        .route("/repos/{owner}/{slug}/branches", get(branches))
        .route("/repos/{owner}/{slug}/commits", get(commits))
        .route("/repos/{owner}/{slug}/commit/{sha}", get(commit_detail))
        .route("/repos/{owner}/{slug}/tree", get(tree))
        .route("/repos/{owner}/{slug}/raw", get(raw))
        .route("/repos/{owner}/{slug}/diff", get(diff))
        .route("/repos/{owner}/{slug}/merge", post(merge))
}
