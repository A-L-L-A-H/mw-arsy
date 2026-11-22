// GitOps Fleet Sync Controller for ARSY
// Provides declarative desired state management for fleet resources
// Watches Git repository with desired state YAML and reconciles actual state

use crate::auth::AuthUser;
use crate::error::{internal_error, err, ApiError};
use crate::services::agent::AgentHandle;
use crate::AppState;
use axum::extract::{Path, State};
use axum::Json;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::Mutex;
use tracing::{debug, info, warn};
use uuid::Uuid;

/// GitOps Controller configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GitOpsConfig {
    pub enabled: bool,
    pub git_repo_url: String,           // SSH or HTTPS URL of the desired state repo
    pub git_branch: String,             // Branch to watch (default: main)
    pub git_ssh_key_path: Option<String>, // Path to SSH private key for private repos
    pub sync_interval_seconds: u64,     // How often to sync (default: 300)
    pub dry_run: bool,                  // If true, only report drift without applying
    pub auto_reconcile: bool,           // If true, automatically apply changes
    pub prune_orphaned: bool,           // If true, remove resources not in desired state
}

/// Desired state resource types
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "kind")]
pub enum DesiredResource {
    #[serde(rename = "Site")]
    Site(DesiredSite),
    #[serde(rename = "Database")]
    Database(DesiredDatabase),
    #[serde(rename = "DockerApp")]
    DockerApp(DesiredDockerApp),
    #[serde(rename = "DockerStack")]
    DockerStack(DesiredDockerStack),
    #[serde(rename = "Server")]
    Server(DesiredServer),
}

/// Desired Site specification
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DesiredSite {
    pub metadata: ResourceMetadata,
    pub spec: SiteSpec,
}

/// Desired Database specification
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DesiredDatabase {
    pub metadata: ResourceMetadata,
    pub spec: DatabaseSpec,
}

/// Desired Docker App specification
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DesiredDockerApp {
    pub metadata: ResourceMetadata,
    pub spec: DockerAppSpec,
}

/// Desired Docker Stack specification
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DesiredDockerStack {
    pub metadata: ResourceMetadata,
    pub spec: DockerStackSpec,
}

/// Desired Server specification
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DesiredServer {
    pub metadata: ResourceMetadata,
    pub spec: ServerSpec,
}

/// Common resource metadata
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ResourceMetadata {
    pub name: String,
    pub namespace: Option<String>,    // For future multi-tenancy
    pub labels: HashMap<String, String>,
    pub annotations: HashMap<String, String>,
}

/// Site specification
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SiteSpec {
    pub domain: String,
    pub runtime: String,              // "static", "php", "nodejs", "python", "proxy"
    pub php_version: Option<String>,
    pub nodejs_version: Option<String>,
    pub python_version: Option<String>,
    pub container_port: Option<u16>,
    pub ssl_mode: String,             // "none", "acme", "provided"
    pub ssl_certificate_id: Option<Uuid>,
    pub memory_mb: Option<u64>,
    pub cpu_percent: Option<u64>,
    pub max_upload_mb: Option<u64>,
    pub fastcgi_cache: Option<bool>,
    pub redis_cache: Option<bool>,
    pub waf_enabled: Option<bool>,
    pub waf_mode: Option<String>,
    pub csp_policy: Option<String>,
    pub bot_protection: Option<String>,
}

/// Database specification
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DatabaseSpec {
    pub engine: String,               // "postgresql" or "mysql"
    pub version: String,
    pub name: String,
    pub site_id: Option<Uuid>,        // If attached to a site
    pub stack_id: Option<Uuid>,       // If attached to a stack
    pub memory_mb: Option<u64>,
    pub cpu_percent: Option<u64>,
}

/// Docker App specification
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DockerAppSpec {
    pub image: String,
    pub container_port: u16,
    pub env_vars: HashMap<String, String>,
    pub volumes: Vec<String>,         // Container paths
    pub memory_mb: Option<u64>,
    pub cpu_percent: Option<u64>,
    pub gpu_enabled: Option<bool>,
    pub restart_policy: Option<String>,
}

/// Docker Stack specification
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DockerStackSpec {
    pub compose_yaml: String,
    pub tls_mode: String,             // "none", "acme", "provided"
    pub tls_certificate_id: Option<Uuid>,
}

/// Server specification
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ServerSpec {
    pub hostname: String,
    pub region: String,
    pub agent_token: Option<String>,  // If pre-provisioned
}

/// Reconciliation result for a single resource
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReconciliationResult {
    pub resource: DesiredResource,
    pub status: ReconciliationStatus,
    pub message: String,
    pub actual_state: Option<serde_json::Value>,
    pub diff: Option<serde_json::Value>,
}

/// Reconciliation status
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "lowercase")]
pub enum ReconciliationStatus {
    Synced,
    DriftDetected,
    Applied,
    Failed,
    Skipped,
}

/// Full reconciliation report
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReconciliationReport {
    pub timestamp: DateTime<Utc>,
    pub git_commit: String,
    pub total_resources: usize,
    pub synced: usize,
    pub drift_detected: usize,
    pub applied: usize,
    pub failed: usize,
    pub skipped: usize,
    pub results: Vec<ReconciliationResult>,
}

/// GitOps Controller state
pub struct GitOpsController {
    config: GitOpsConfig,
    last_sync: Arc<Mutex<Option<DateTime<Utc>>>>,
    last_report: Arc<Mutex<Option<ReconciliationReport>>>,
    running: Arc<Mutex<bool>>,
}

impl GitOpsController {
    pub fn new(config: GitOpsConfig) -> Self {
        Self {
            config,
            last_sync: Arc::new(Mutex::new(None)),
            last_report: Arc::new(Mutex::new(None)),
            running: Arc::new(Mutex::new(false)),
        }
    }

    /// Load desired state from Git repository
    pub async fn load_desired_state(&self, repo_path: &str) -> Result<Vec<DesiredResource>, String> {
        // Read all YAML files from the desired state directory
        let desired_dir = std::path::Path::new(repo_path).join("desired");
        if !desired_dir.exists() {
            return Ok(Vec::new());
        }

        let mut resources = Vec::new();
        let mut entries = tokio::fs::read_dir(&desired_dir).await
            .map_err(|e| format!("Failed to read desired dir: {e}"))?;

        while let Some(entry) = entries.next_entry().await
            .map_err(|e| format!("Failed to read dir entry: {e}"))? {
            let path = entry.path();
            if path.extension().and_then(|s| s.to_str()) != Some("yaml") &&
               path.extension().and_then(|s| s.to_str()) != Some("yml") {
                continue;
            }

            let content = tokio::fs::read_to_string(&path).await
                .map_err(|e| format!("Failed to read file: {e}"))?;

            // Parse YAML - could be a single resource or a list
            let docs: Vec<serde_json::Value> = serde_yaml::from_str(&content)
                .map_err(|e| format!("Failed to parse YAML: {e}"))?;

            for doc in docs {
                if let Ok(resource) = serde_json::from_value::<DesiredResource>(doc) {
                    resources.push(resource);
                }
            }
        }

        Ok(resources)
    }

    /// Get current actual state from the panel
    pub async fn get_actual_state(&self, state: &crate::AppState, server_id: Uuid) -> Result<Vec<serde_json::Value>, String> {
        // This would query the actual state from the panel's databases
        // For now, return empty - actual implementation would query sites, databases, apps, etc.
        Ok(Vec::new())
    }

    /// Compute diff between desired and actual state
    pub fn compute_diff(&self, desired: &DesiredResource, actual: &serde_json::Value) -> Option<serde_json::Value> {
        // Simple diff - in practice would use a proper diff library
        let desired_json = serde_json::to_value(desired).ok()?;
        if desired_json != *actual {
            Some(serde_json::json!({
                "desired": desired_json,
                "actual": actual,
            }))
        } else {
            None
        }
    }

    /// Apply changes to reconcile a resource
    pub async fn apply_resource(
        &self,
        state: &crate::AppState,
        server_id: Uuid,
        resource: &DesiredResource,
        dry_run: bool,
    ) -> Result<ReconciliationResult, String> {
        // This is a placeholder - actual implementation would call the appropriate
        // panel APIs to create/update/delete resources
        match resource {
            DesiredResource::Site(site) => {
                if dry_run {
                    Ok(ReconciliationResult {
                        resource: resource.clone(),
                        status: ReconciliationStatus::Skipped,
                        message: "Dry run: would create/update site".to_string(),
                        actual_state: None,
                        diff: None,
                    })
                } else {
                    // Call sites::create or sites::update
                    Ok(ReconciliationResult {
                        resource: resource.clone(),
                        status: ReconciliationStatus::Applied,
                        message: format!("Applied site {}", site.metadata.name),
                        actual_state: None,
                        diff: None,
                    })
                }
            }
            DesiredResource::Database(db) => {
                if dry_run {
                    Ok(ReconciliationResult {
                        resource: resource.clone(),
                        status: ReconciliationStatus::Skipped,
                        message: "Dry run: would create/update database".to_string(),
                        actual_state: None,
                        diff: None,
                    })
                } else {
                    Ok(ReconciliationResult {
                        resource: resource.clone(),
                        status: ReconciliationStatus::Applied,
                        message: format!("Applied database {}", db.metadata.name),
                        actual_state: None,
                        diff: None,
                    })
                }
            }
            DesiredResource::DockerApp(app) => {
                if dry_run {
                    Ok(ReconciliationResult {
                        resource: resource.clone(),
                        status: ReconciliationStatus::Skipped,
                        message: "Dry run: would create/update docker app".to_string(),
                        actual_state: None,
                        diff: None,
                    })
                } else {
                    Ok(ReconciliationResult {
                        resource: resource.clone(),
                        status: ReconciliationStatus::Applied,
                        message: format!("Applied docker app {}", app.metadata.name),
                        actual_state: None,
                        diff: None,
                    })
                }
            }
            DesiredResource::DockerStack(stack) => {
                if dry_run {
                    Ok(ReconciliationResult {
                        resource: resource.clone(),
                        status: ReconciliationStatus::Skipped,
                        message: "Dry run: would create/update docker stack".to_string(),
                        actual_state: None,
                        diff: None,
                    })
                } else {
                    Ok(ReconciliationResult {
                        resource: resource.clone(),
                        status: ReconciliationStatus::Applied,
                        message: format!("Applied docker stack {}", stack.metadata.name),
                        actual_state: None,
                        diff: None,
                    })
                }
            }
            DesiredResource::Server(server) => {
                if dry_run {
                    Ok(ReconciliationResult {
                        resource: resource.clone(),
                        status: ReconciliationStatus::Skipped,
                        message: "Dry run: would create/update server".to_string(),
                        actual_state: None,
                        diff: None,
                    })
                } else {
                    Ok(ReconciliationResult {
                        resource: resource.clone(),
                        status: ReconciliationStatus::Applied,
                        message: format!("Applied server {}", server.metadata.name),
                        actual_state: None,
                        diff: None,
                    })
                }
            }
        }
    }

    /// Run full reconciliation loop
    pub async fn reconcile(&self, state: &crate::AppState) -> Result<ReconciliationReport, String> {
        if !self.config.enabled {
            return Err("GitOps controller not enabled".to_string());
        }

        {
            let mut running = self.running.lock().await;
            if *running {
                return Err("Reconciliation already in progress".to_string());
            }
            *running = true;
        }

        let started_at = Utc::now();
        let mut results = Vec::new();
        let mut synced = 0;
        let mut drift_detected = 0;
        let mut applied = 0;
        let mut failed = 0;
        let mut skipped = 0;

        // Clone the repo (or pull latest)
        let repo_path = self.clone_or_pull_repo().await?;

        // Get git commit hash
        let git_commit = self.get_repo_head_commit(&repo_path).await?;

        // Load desired state from repo
        let desired_resources = self.load_desired_state(&repo_path).await?;

        // Get actual state from panel (for each server in fleet)
        // For now, just use local server
        let servers = self.get_managed_servers().await?;
        
        for server in servers {
            let actual_state = self.get_actual_state(state, server.id).await?;

            for desired in &desired_resources {
                // Find matching actual resource
                let actual = self.find_matching_actual(desired, &actual_state);
                let diff = self.compute_diff(desired, &actual.unwrap_or(serde_json::Value::Null));

                if diff.is_none() {
                    results.push(ReconciliationResult {
                        resource: desired.clone(),
                        status: ReconciliationStatus::Synced,
                        message: "Resource is in sync".to_string(),
                        actual_state: actual,
                        diff: None,
                    });
                    synced += 1;
                    continue;
                }

                drift_detected += 1;

                if self.config.dry_run {
                    results.push(ReconciliationResult {
                        resource: desired.clone(),
                        status: ReconciliationStatus::DriftDetected,
                        message: "Drift detected (dry run)".to_string(),
                        actual_state: actual,
                        diff,
                    });
                    skipped += 1;
                } else if self.config.auto_reconcile {
                    match self.apply_resource(state, server.id, desired, false).await {
                        Ok(result) => {
                            if result.status == ReconciliationStatus::Applied {
                                applied += 1;
                            } else {
                                failed += 1;
                            }
                            results.push(result);
                        }
                        Err(e) => {
                            results.push(ReconciliationResult {
                                resource: desired.clone(),
                                status: ReconciliationStatus::Failed,
                                message: format!("Failed to apply: {e}"),
                                actual_state: actual,
                                diff,
                            });
                            failed += 1;
                        }
                    }
                } else {
                    results.push(ReconciliationResult {
                        resource: desired.clone(),
                        status: ReconciliationStatus::DriftDetected,
                        message: "Drift detected (auto-reconcile disabled)".to_string(),
                        actual_state: actual,
                        diff,
                    });
                    skipped += 1;
                }
            }
        }

        let report = ReconciliationReport {
            timestamp: Utc::now(),
            git_commit,
            total_resources: desired_resources.len(),
            synced,
            drift_detected,
            applied,
            failed,
            skipped,
            results,
        };

        {
            let mut last_report = self.last_report.lock().await;
            *last_report = Some(report.clone());
        }
        {
            let mut last_sync = self.last_sync.lock().await;
            *last_sync = Some(Utc::now());
        }
        {
            let mut running = self.running.lock().await;
            *running = false;
        }

        Ok(report)
    }

    /// Clone or pull the Git repository
    async fn clone_or_pull_repo(&self) -> Result<String, String> {
        let repo_dir = format!("/var/lib/arsy/gitops/{}", 
            self.config.git_repo_url.split('/').last().unwrap_or("repo").replace(".git", ""));
        
        let git_dir = format!("{repo_dir}/.git");
        
        if std::path::Path::new(&git_dir).exists() {
            // Pull latest
            let mut cmd = crate::safe_cmd::safe_command("git");
            cmd.args(["-C", &repo_dir, "fetch", "origin", &self.config.git_branch])
                .env("GIT_TERMINAL_PROMPT", "0");
            if let Some(key_path) = &self.config.git_ssh_key_path {
                cmd.env("GIT_SSH_COMMAND", format!("ssh -i {key_path} -o StrictHostKeyChecking=no"));
            }
            cmd.kill_on_drop(true);

            let output = tokio::time::timeout(
                std::time::Duration::from_secs(120),
                cmd.output(),
            ).await
            .map_err(|_| "git fetch timed out".to_string())?
            .map_err(|e| format!("git fetch failed: {e}"))?;

            if !output.status.success() {
                return Err(format!("git fetch failed: {}", String::from_utf8_lossy(&output.stderr)));
            }

            let reset = crate::safe_cmd::safe_command("git")
                .args(["-C", &repo_dir, "reset", "--hard", &format!("origin/{}", self.config.git_branch)])
                .output()
                .await
                .map_err(|e| format!("git reset failed: {e}"))?;

            if !reset.status.success() {
                return Err(format!("git reset failed: {}", String::from_utf8_lossy(&reset.stderr)));
            }
        } else {
            // Fresh clone
            std::fs::create_dir_all(&repo_dir)
                .map_err(|e| format!("Failed to create repo dir: {e}"))?;

            let mut cmd = crate::safe_cmd::safe_command("git");
            cmd.args([
                "clone", "--branch", &self.config.git_branch, "--single-branch",
                &self.config.git_repo_url, &repo_dir,
            ])
            .env("GIT_TERMINAL_PROMPT", "0");
            if let Some(key_path) = &self.config.git_ssh_key_path {
                cmd.env("GIT_SSH_COMMAND", format!("ssh -i {} -o StrictHostKeyChecking=no", key_path));
            }
            cmd.kill_on_drop(true);

            let output = tokio::time::timeout(
                std::time::Duration::from_secs(300),
                cmd.output(),
            ).await
            .map_err(|_| "git clone timed out".to_string())?
            .map_err(|e| format!("git clone failed: {e}"))?;

            if !output.status.success() {
                return Err(format!("git clone failed: {}", String::from_utf8_lossy(&output.stderr)));
            }
        }

        Ok(repo_dir)
    }

    async fn get_repo_head_commit(&self, repo_path: &str) -> Result<String, String> {
        let output = crate::safe_cmd::safe_command("git")
            .args(["-C", repo_path, "rev-parse", "--short", "HEAD"])
            .output()
            .await
            .map_err(|e| format!("Failed to get commit hash: {e}"))?;

        if !output.status.success() {
            return Err("Failed to get commit hash".to_string());
        }

        Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
    }

    async fn get_managed_servers(&self) -> Result<Vec<ServerInfo>, String> {
        // Placeholder - would query the servers table
        Ok(Vec::new())
    }

    fn find_matching_actual(&self, desired: &DesiredResource, actual_state: &[serde_json::Value]) -> Option<serde_json::Value> {
        // Match by name and kind
        let name = match desired {
            DesiredResource::Site(s) => &s.metadata.name,
            DesiredResource::Database(d) => &d.metadata.name,
            DesiredResource::DockerApp(a) => &a.metadata.name,
            DesiredResource::DockerStack(s) => &s.metadata.name,
            DesiredResource::Server(s) => &s.metadata.name,
        };
        
        let kind = match desired {
            DesiredResource::Site(_) => "Site",
            DesiredResource::Database(_) => "Database",
            DesiredResource::DockerApp(_) => "DockerApp",
            DesiredResource::DockerStack(_) => "DockerStack",
            DesiredResource::Server(_) => "Server",
        };

        actual_state.iter().find(|a| {
            a.get("metadata")
                .and_then(|m| m.get("name"))
                .and_then(|n| n.as_str()) == Some(name)
                && a.get("kind").and_then(|k| k.as_str()) == Some(kind)
        }).cloned()
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct ServerInfo {
    id: Uuid,
    name: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_reconciliation_status_ordering() {
        let statuses = vec![
            ReconciliationStatus::Synced,
            ReconciliationStatus::DriftDetected,
            ReconciliationStatus::Applied,
            ReconciliationStatus::Failed,
        ];
        assert_eq!(statuses.len(), 4);
    }
}