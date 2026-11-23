use axum::{
    body::Body,
    extract::{Path, Query, State},
    http::{header, HeaderMap, HeaderValue, StatusCode},
    response::{IntoResponse, Response},
    routing::{get, post},
    Router,
};
use base64::{engine::general_purpose::STANDARD, Engine};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::path::Path as StdPath;
use std::process::Stdio;
use std::sync::{Mutex as StdMutex, OnceLock};
use std::time::{Duration, Instant};
use tokio::io::AsyncWriteExt;
use tokio_stream::StreamExt;
use tokio_util::io::ReaderStream;

use crate::routes::AppState;
use crate::safe_cmd::safe_command;
use crate::services::git_repos::{
    commit_messages_in_range, normalize_slug, repo_path, GIT_REPOS_ROOT,
};
use crate::site_lock::lock_site;

const AUTH_CACHE_TTL: Duration = Duration::from_secs(60);
const MAX_REF_COMMAND_HEADER_BYTES: usize = 256 * 1024;

#[derive(Debug, Deserialize)]
pub struct InfoRefsQuery {
    pub service: Option<String>,
}

#[derive(Debug, Clone)]
struct CachedAuth {
    role: String,
    protect_default: bool,
    default_branch: String,
    repo_id: String,
    user_id: String,
    expires_at: Instant,
}

static AUTH_CACHE: OnceLock<StdMutex<HashMap<String, CachedAuth>>> = OnceLock::new();

fn auth_cache() -> &'static StdMutex<HashMap<String, CachedAuth>> {
    AUTH_CACHE.get_or_init(|| StdMutex::new(HashMap::new()))
}

#[derive(Debug, Serialize)]
struct InternalAuthRequest<'a> {
    owner: &'a str,
    slug: &'a str,
    token_hash: &'a str,
    required_op: &'a str,
}

#[derive(Debug, Deserialize)]
struct InternalAuthResponse {
    allowed: bool,
    role: String,
    protect_default: bool,
    default_branch: String,
    repo_id: String,
    user_id: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct RefUpdate {
    pub ref_name: String,
    pub old_sha: String,
    pub new_sha: String,
}

#[derive(Debug, Serialize)]
struct InternalPushCommit {
    sha: String,
    message: String,
}

#[derive(Debug, Serialize)]
struct InternalPushNotification {
    repo_id: String,
    owner: String,
    slug: String,
    user_id: String,
    updates: Vec<RefUpdate>,
    commits: Vec<InternalPushCommit>,
}

fn backend_base_url() -> String {
    std::env::var("BACKEND_INTERNAL_URL")
        .or_else(|_| std::env::var("ARSY_CENTRAL_URL"))
        .unwrap_or_else(|_| "http://127.0.0.1:3080".to_string())
        .trim_end_matches('/')
        .to_string()
}

fn http_client() -> &'static reqwest::Client {
    static CLIENT: OnceLock<reqwest::Client> = OnceLock::new();
    CLIENT.get_or_init(|| {
        reqwest::Client::builder()
            .timeout(Duration::from_secs(10))
            .build()
            .expect("git_http reqwest client")
    })
}

fn extract_git_token(headers: &HeaderMap) -> Option<String> {
    if let Some(tok) = headers.get("x-git-token").and_then(|v| v.to_str().ok()) {
        let trimmed = tok.trim();
        if !trimmed.is_empty() {
            return Some(trimmed.to_string());
        }
    }
    let auth = headers.get(header::AUTHORIZATION).and_then(|v| v.to_str().ok())?;
    if let Some(b) = auth.strip_prefix("Basic ").or_else(|| auth.strip_prefix("basic ")) {
        let bytes = STANDARD.decode(b.trim()).ok()?;
        let decoded = String::from_utf8(bytes).ok()?;
        if let Some((_u, p)) = decoded.split_once(':') {
            if !p.is_empty() {
                return Some(p.to_string());
            }
        }
        if !decoded.is_empty() {
            return Some(decoded);
        }
    }
    if let Some(tok) = auth.strip_prefix("Bearer ") {
        if tok.starts_with("dpgit_") {
            return Some(tok.to_string());
        }
    }
    None
}

fn hash_token(raw: &str) -> String {
    let mut h = Sha256::new();
    h.update(raw.as_bytes());
    hex::encode(h.finalize())
}

/// Authorize a git transport request against the backend (`POST /internal/git/auth`).
/// Positive results are cached 60s; negative results and backend outages fail closed.
async fn verify_git_access(
    state: &AppState,
    headers: &HeaderMap,
    owner: &str,
    slug: &str,
    required_op: &str,
) -> Result<CachedAuth, Response> {
    let Some(raw_token) = extract_git_token(headers) else {
        return Err(unauthorized_basic());
    };
    let token_hash = hash_token(&raw_token);
    let cache_key = format!("{token_hash}:{owner}:{slug}:{required_op}");

    if let Some(hit) = {
        let mut guard = auth_cache().lock().unwrap_or_else(|p| p.into_inner());
        if let Some(entry) = guard.get(&cache_key) {
            if entry.expires_at > Instant::now() {
                Some(entry.clone())
            } else {
                guard.remove(&cache_key);
                None
            }
        } else {
            None
        }
    } {
        return Ok(hit);
    }

    let agent_secret = state.token.read().await.clone();
    let url = format!("{}/internal/git/auth", backend_base_url());
    let resp = http_client()
        .post(&url)
        .header("x-agent-secret", &agent_secret)
        .json(&InternalAuthRequest {
            owner,
            slug,
            token_hash: &token_hash,
            required_op,
        })
        .send()
        .await;

    let resp = match resp {
        Ok(r) => r,
        Err(e) => {
            tracing::error!("Git auth backend call failed closed: {e}");
            return Err((StatusCode::SERVICE_UNAVAILABLE, "Git auth service unavailable").into_response());
        }
    };

    if resp.status() == reqwest::StatusCode::UNAUTHORIZED {
        let mut guard = auth_cache().lock().unwrap_or_else(|p| p.into_inner());
        guard.remove(&cache_key);
        return Err(unauthorized_basic());
    }
    if resp.status() == reqwest::StatusCode::FORBIDDEN {
        return Err((StatusCode::FORBIDDEN, "Permission denied").into_response());
    }
    if resp.status() == reqwest::StatusCode::NOT_FOUND {
        return Err((StatusCode::NOT_FOUND, "Repository not found").into_response());
    }
    if !resp.status().is_success() {
        tracing::error!("Git auth backend returned {}", resp.status());
        return Err((StatusCode::BAD_GATEWAY, "Git auth verification failed").into_response());
    }

    let parsed: InternalAuthResponse = match resp.json().await {
        Ok(p) => p,
        Err(e) => {
            tracing::error!("Malformed git auth response: {e}");
            return Err((StatusCode::BAD_GATEWAY, "Malformed auth response").into_response());
        }
    };

    if !parsed.allowed {
        return Err((StatusCode::FORBIDDEN, "Permission denied").into_response());
    }

    let entry = CachedAuth {
        role: parsed.role,
        protect_default: parsed.protect_default,
        default_branch: parsed.default_branch,
        repo_id: parsed.repo_id,
        user_id: parsed.user_id,
        expires_at: Instant::now() + AUTH_CACHE_TTL,
    };

    {
        let mut guard = auth_cache().lock().unwrap_or_else(|p| p.into_inner());
        if guard.len() > 2048 {
            let now = Instant::now();
            guard.retain(|_, v| v.expires_at > now);
        }
        guard.insert(cache_key, entry.clone());
    }

    Ok(entry)
}

fn unauthorized_basic() -> Response {
    let mut resp = (StatusCode::UNAUTHORIZED, "Authentication required").into_response();
    resp.headers_mut().insert(
        header::WWW_AUTHENTICATE,
        HeaderValue::from_static(r#"Basic realm="ARSY Git""#),
    );
    resp
}

fn pkt_line(data: &[u8]) -> Vec<u8> {
    let len = data.len() + 4;
    let mut out = format!("{len:04x}").into_bytes();
    out.extend_from_slice(data);
    out
}

fn pkt_sideband_error(ref_name: &str, msg: &str) -> Vec<u8> {
    let mut out = Vec::new();
    out.extend_from_slice(&pkt_line(b"unpack ok\n"));
    out.extend_from_slice(&pkt_line(format!("ng {ref_name} {msg}\n").as_bytes()));
    out.extend_from_slice(b"0000");
    out
}

/// Parse pkt-line ref-update header lines up to the first flush `0000`.
/// Returns `(updates, remaining_bytes_after_flush)`.
fn parse_receive_pack_header(buf: &[u8]) -> Result<(Vec<RefUpdate>, usize), String> {
    let mut pos = 0usize;
    let mut updates = Vec::new();
    while pos + 4 <= buf.len() {
        let len_hex = std::str::from_utf8(&buf[pos..pos + 4]).map_err(|_| "invalid pkt-line")?;
        if len_hex == "0000" {
            return Ok((updates, pos + 4));
        }
        let pkt_len = usize::from_str_radix(len_hex, 16).map_err(|_| "invalid pkt length")?;
        if pkt_len < 4 || pos + pkt_len > buf.len() {
            return Err("incomplete pkt-line".to_string());
        }
        let line_bytes = &buf[pos + 4..pos + pkt_len];
        pos += pkt_len;

        // Strip capabilities after NUL byte on the first ref line
        let line_part = line_bytes.split(|&b| b == 0).next().unwrap_or(line_bytes);
        let line_str = String::from_utf8_lossy(line_part).trim().to_string();
        let mut cols = line_str.split_whitespace();
        if let (Some(old_sha), Some(new_sha), Some(ref_name)) =
            (cols.next(), cols.next(), cols.next())
        {
            updates.push(RefUpdate {
                ref_name: ref_name.to_string(),
                old_sha: old_sha.to_string(),
                new_sha: new_sha.to_string(),
            });
        }
    }
    Err("missing flush packet".to_string())
}

/// GET /git-server/{owner}/{slug}/info/refs?service=git-upload-pack|git-receive-pack
pub async fn info_refs(
    State(state): State<AppState>,
    Path((owner, raw_slug)): Path<(String, String)>,
    Query(query): Query<InfoRefsQuery>,
    headers: HeaderMap,
) -> Response {
    let slug = normalize_slug(&raw_slug).to_string();
    let Some(service) = query.service.as_deref() else {
        return (StatusCode::BAD_REQUEST, "Missing service query parameter").into_response();
    };
    let (subcmd, required_op) = match service {
        "git-upload-pack" => ("upload-pack", "read"),
        "git-receive-pack" => ("receive-pack", "write"),
        _ => return (StatusCode::FORBIDDEN, "Unsupported git service").into_response(),
    };

    if let Err(resp) = verify_git_access(&state, &headers, &owner, &slug, required_op).await {
        return resp;
    }

    let path = match repo_path(StdPath::new(GIT_REPOS_ROOT), &owner, &slug) {
        Ok(p) if p.exists() => p,
        Ok(_) => return (StatusCode::NOT_FOUND, "Repository not found").into_response(),
        Err(e) => return (StatusCode::BAD_REQUEST, e).into_response(),
    };

    let path_str = path.to_string_lossy().to_string();
    let mut cmd = safe_command("git");
    cmd.args([subcmd, "--stateless-rpc", "--advertise-refs", &path_str]);
    cmd.kill_on_drop(true);
    let out = match cmd.output().await {
        Ok(o) if o.status.success() => o,
        Ok(o) => {
            let err = String::from_utf8_lossy(&o.stderr).to_string();
            return (StatusCode::INTERNAL_SERVER_ERROR, err).into_response();
        }
        Err(e) => return (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
    };

    let mut payload = Vec::with_capacity(64 + out.stdout.len());
    payload.extend_from_slice(&pkt_line(format!("# service={service}\n").as_bytes()));
    payload.extend_from_slice(b"0000");
    payload.extend_from_slice(&out.stdout);

    let ct = format!("application/x-{service}-advertisement");
    Response::builder()
        .status(StatusCode::OK)
        .header(header::CONTENT_TYPE, ct)
        .header(header::CACHE_CONTROL, "no-cache")
        .body(Body::from(payload))
        .unwrap_or_else(|_| StatusCode::INTERNAL_SERVER_ERROR.into_response())
}

/// POST /git-server/{owner}/{slug}/git-upload-pack
pub async fn upload_pack(
    State(state): State<AppState>,
    Path((owner, raw_slug)): Path<(String, String)>,
    headers: HeaderMap,
    body: Body,
) -> Response {
    let slug = normalize_slug(&raw_slug).to_string();
    if let Err(resp) = verify_git_access(&state, &headers, &owner, &slug, "read").await {
        return resp;
    }
    let path = match repo_path(StdPath::new(GIT_REPOS_ROOT), &owner, &slug) {
        Ok(p) if p.exists() => p,
        Ok(_) => return (StatusCode::NOT_FOUND, "Repository not found").into_response(),
        Err(e) => return (StatusCode::BAD_REQUEST, e).into_response(),
    };

    stream_git_rpc("upload-pack", "application/x-git-upload-pack-result", &path, Vec::new(), body).await
}

/// POST /git-server/{owner}/{slug}/git-receive-pack
pub async fn receive_pack(
    State(state): State<AppState>,
    Path((owner, raw_slug)): Path<(String, String)>,
    headers: HeaderMap,
    body: Body,
) -> Response {
    let slug = normalize_slug(&raw_slug).to_string();
    let auth = match verify_git_access(&state, &headers, &owner, &slug, "write").await {
        Ok(a) => a,
        Err(resp) => return resp,
    };
    let path = match repo_path(StdPath::new(GIT_REPOS_ROOT), &owner, &slug) {
        Ok(p) if p.exists() => p,
        Ok(_) => return (StatusCode::NOT_FOUND, "Repository not found").into_response(),
        Err(e) => return (StatusCode::BAD_REQUEST, e).into_response(),
    };

    // Peek only the small ref-update header (up to flush `0000`), leaving packfile
    // payload in the stream so multi-GB pushes never buffer in RAM.
    let mut stream = body.into_data_stream();
    let mut header_buf = Vec::new();
    let mut parsed_updates: Option<Vec<RefUpdate>> = None;

    while let Some(chunk_res) = stream.next().await {
        let chunk = match chunk_res {
            Ok(c) => c,
            Err(e) => return (StatusCode::BAD_REQUEST, format!("Stream error: {e}")).into_response(),
        };
        header_buf.extend_from_slice(&chunk);
        if header_buf.len() > MAX_REF_COMMAND_HEADER_BYTES {
            return (StatusCode::PAYLOAD_TOO_LARGE, "Ref-update header exceeds limit").into_response();
        }
        if let Ok((updates, _flush_end)) = parse_receive_pack_header(&header_buf) {
            parsed_updates = Some(updates);
            break;
        }
    }

    let updates = parsed_updates.unwrap_or_default();

    // Enforce protected default branch rule before spawning receive-pack
    if auth.protect_default && !auth.default_branch.is_empty() {
        let protected_ref = format!("refs/heads/{}", auth.default_branch);
        if updates.iter().any(|u| u.ref_name == protected_ref) {
            let err_body = pkt_sideband_error(
                &protected_ref,
                "protected branch: direct pushes are blocked; open a Pull Request",
            );
            return Response::builder()
                .status(StatusCode::OK)
                .header(header::CONTENT_TYPE, "application/x-git-receive-pack-result")
                .header(header::CACHE_CONTROL, "no-cache")
                .body(Body::from(err_body))
                .unwrap_or_else(|_| StatusCode::INTERNAL_SERVER_ERROR.into_response());
        }
    }

    // Serialize ref updates per repository
    let _repo_guard = lock_site(&format!("git-repo:{owner}/{slug}")).await;

    let path_str = path.to_string_lossy().to_string();
    let mut cmd = safe_command("git");
    cmd.args(["receive-pack", "--stateless-rpc", &path_str])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    cmd.kill_on_drop(true);

    let mut child = match cmd.spawn() {
        Ok(c) => c,
        Err(e) => return (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
    };

    let Some(mut stdin) = child.stdin.take() else {
        return (StatusCode::INTERNAL_SERVER_ERROR, "Missing child stdin").into_response();
    };
    let Some(stdout) = child.stdout.take() else {
        return (StatusCode::INTERNAL_SERVER_ERROR, "Missing child stdout").into_response();
    };

    tokio::spawn(async move {
        if !header_buf.is_empty() && stdin.write_all(&header_buf).await.is_err() {
            return;
        }
        while let Some(chunk_res) = stream.next().await {
            match chunk_res {
                Ok(c) => {
                    if stdin.write_all(&c).await.is_err() {
                        break;
                    }
                }
                Err(_) => break,
            }
        }
        let _ = stdin.shutdown().await;
    });

    // Collect receive-pack status report (small pkt-line report) and await exit code
    // so we only notify backend on a successful push.
    let mut out_reader = ReaderStream::new(stdout);
    let mut out_bytes = Vec::new();
    while let Some(chunk) = out_reader.next().await {
        if let Ok(b) = chunk {
            out_bytes.extend_from_slice(&b);
        }
    }
    let status = child.wait().await;
    if let Ok(st) = status {
        if st.success() && !updates.is_empty() {
            let agent_secret = state.token.read().await.clone();
            let root = StdPath::new(GIT_REPOS_ROOT);
            let mut commits = Vec::new();
            for u in &updates {
                if let Ok(msgs) =
                    commit_messages_in_range(root, &owner, &slug, &u.old_sha, &u.new_sha).await
                {
                    for (sha, message) in msgs {
                        commits.push(InternalPushCommit { sha, message });
                    }
                }
            }
            let notif = InternalPushNotification {
                repo_id: auth.repo_id,
                owner,
                slug,
                user_id: auth.user_id,
                updates,
                commits,
            };
            tokio::spawn(async move {
                let url = format!("{}/internal/git/push", backend_base_url());
                if let Err(e) = http_client()
                    .post(&url)
                    .header("x-agent-secret", &agent_secret)
                    .json(&notif)
                    .send()
                    .await
                {
                    tracing::warn!("Failed to emit internal git push notification: {e}");
                }
            });
        }
    }

    Response::builder()
        .status(StatusCode::OK)
        .header(header::CONTENT_TYPE, "application/x-git-receive-pack-result")
        .header(header::CACHE_CONTROL, "no-cache")
        .body(Body::from(out_bytes))
        .unwrap_or_else(|_| StatusCode::INTERNAL_SERVER_ERROR.into_response())
}

async fn stream_git_rpc(
    subcmd: &str,
    content_type: &str,
    path: &StdPath,
    prefix: Vec<u8>,
    body: Body,
) -> Response {
    let path_str = path.to_string_lossy().to_string();
    let mut cmd = safe_command("git");
    cmd.args([subcmd, "--stateless-rpc", &path_str])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    cmd.kill_on_drop(true);

    let mut child = match cmd.spawn() {
        Ok(c) => c,
        Err(e) => return (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
    };

    let Some(mut stdin) = child.stdin.take() else {
        return (StatusCode::INTERNAL_SERVER_ERROR, "Missing child stdin").into_response();
    };
    let Some(stdout) = child.stdout.take() else {
        return (StatusCode::INTERNAL_SERVER_ERROR, "Missing child stdout").into_response();
    };

    let mut in_stream = body.into_data_stream();
    tokio::spawn(async move {
        if !prefix.is_empty() && stdin.write_all(&prefix).await.is_err() {
            return;
        }
        while let Some(chunk_res) = in_stream.next().await {
            match chunk_res {
                Ok(c) => {
                    if stdin.write_all(&c).await.is_err() {
                        break;
                    }
                }
                Err(_) => break,
            }
        }
        let _ = stdin.shutdown().await;
    });

    // Keep child handle alive until stdout stream finishes
    tokio::spawn(async move {
        let _ = child.wait().await;
    });

    let out_stream = ReaderStream::new(stdout);
    Response::builder()
        .status(StatusCode::OK)
        .header(header::CONTENT_TYPE, content_type)
        .header(header::CACHE_CONTROL, "no-cache")
        .body(Body::from_stream(out_stream))
        .unwrap_or_else(|_| StatusCode::INTERNAL_SERVER_ERROR.into_response())
}

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/git-server/{owner}/{slug}/info/refs", get(info_refs))
        .route("/git-server/{owner}/{slug}/git-upload-pack", post(upload_pack))
        .route("/git-server/{owner}/{slug}/git-receive-pack", post(receive_pack))
}
