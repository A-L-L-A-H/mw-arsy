// MCP-Grounded Root Cause Analysis Agent for ARSY Fleet
// Inspired by ARGUS (arXiv:2608.23084) - uses MCP tools to correlate
// metrics, logs, container state, alerts, and incidents for diagnosis

use crate::services::activity;
use crate::error::{internal_error, err, ApiError};
use crate::AppState;
use axum::extract::State;
use axum::Json;
use chrono::{DateTime, Duration, Utc};
use reqwest;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::Mutex;
use tracing::{debug, info, warn};
use uuid::Uuid;

/// RCA Agent configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RcaConfig {
    pub enabled: bool,
    pub mcp_server_url: String,      // e.g., "http://127.0.0.1:3081/mcp"
    pub mcp_api_key: String,         // dp_ key for MCP authentication
    pub auto_trigger_on_alert: bool,
    pub auto_trigger_on_deploy_failure: bool,
    pub max_diagnosis_history: usize,
    pub llm_provider: String,        // "anthropic", "openai", "gemini", "grok"
    pub llm_model: String,
    pub llm_api_key: String,
}

/// Diagnosis request from user or auto-trigger
#[derive(Debug, Deserialize)]
pub struct DiagnosisRequest {
    pub trigger: DiagnosisTrigger,
    pub context: Option<String>,           // Free-form context from user
    pub target_server_id: Option<Uuid>,    // Specific server, or None for all
    pub time_range_hours: Option<i64>,     // How far back to look
}

/// What triggered this diagnosis
#[derive(Debug, Deserialize, Serialize, Clone, PartialEq)]
#[serde(tag = "type")]
pub enum DiagnosisTrigger {
    Manual { description: String },
    AlertFired { alert_id: Uuid, alert_type: String },
    DeployFailed { deploy_id: Uuid, commit_hash: String },
    Scheduled { schedule_name: String },
}

/// Result of a diagnosis
#[derive(Debug, Serialize)]
pub struct DiagnosisResult {
    pub diagnosis_id: Uuid,
    pub trigger: DiagnosisTrigger,
    pub status: DiagnosisStatus,
    pub root_cause: Option<String>,
    pub recommended_actions: Vec<RecommendedAction>,
    pub evidence: Vec<EvidenceItem>,
    pub confidence: f32,            // 0.0 - 1.0
    pub started_at: DateTime<Utc>,
    pub completed_at: Option<DateTime<Utc>>,
    pub llm_tokens_used: Option<u32>,
}

/// Status of diagnosis
#[derive(Debug, Serialize, Deserialize, Clone, PartialEq)]
#[serde(rename_all = "lowercase")]
pub enum DiagnosisStatus {
    Pending,
    Running,
    Completed,
    Failed,
}

/// Recommended action for the operator
#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct RecommendedAction {
    pub action_type: ActionType,
    pub description: String,
    pub command: Option<String>,      // Shell command to run
    pub mcp_tool: Option<String>,     // MCP tool to call
    pub mcp_args: Option<serde_json::Value>,
    pub priority: u8,                 // 1 = highest
    pub estimated_impact: String,
}

/// Type of recommended action
#[derive(Debug, Serialize, Deserialize, Clone, PartialEq)]
#[serde(rename_all = "lowercase")]
pub enum ActionType {
    RestartService,
    ScaleService,
    CheckLogs,
    CheckMetrics,
    RunCommand,
    CallMcpTool,
    UpdateConfig,
    ContactSupport,
}

/// Evidence collected during diagnosis
#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct EvidenceItem {
    pub source: EvidenceSource,
    pub timestamp: DateTime<Utc>,
    pub summary: String,
    pub details: serde_json::Value,
    pub severity: EvidenceSeverity,
}

/// Source of evidence
#[derive(Debug, Serialize, Deserialize, Clone, PartialEq)]
#[serde(rename_all = "lowercase")]
pub enum EvidenceSource {
    Metrics,
    Logs,
    ContainerState,
    Alert,
    DeployHistory,
    SecurityScan,
    McpTool,
}

/// Severity of evidence
#[derive(Debug, Serialize, Deserialize, Clone, PartialEq)]
#[serde(rename_all = "lowercase")]
pub enum EvidenceSeverity {
    Info,
    Warning,
    Critical,
}

/// Historical diagnosis record
#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct DiagnosisRecord {
    pub id: Uuid,
    pub trigger: DiagnosisTrigger,
    pub status: DiagnosisStatus,
    pub root_cause: Option<String>,
    pub recommended_actions: Vec<RecommendedAction>,
    pub evidence: Vec<EvidenceItem>,
    pub confidence: f32,
    pub started_at: DateTime<Utc>,
    pub completed_at: Option<DateTime<Utc>>,
    pub feedback: Option<DiagnosisFeedback>,
}

/// Operator feedback on diagnosis quality
#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct DiagnosisFeedback {
    pub rating: u8,                    // 1-5
    pub accurate: bool,
    pub comments: Option<String>,
    pub submitted_at: DateTime<Utc>,
}

/// RCA Agent state
pub struct RcaAgent {
    config: RcaConfig,
    mcp_client: McpClient,
    history: Arc<Mutex<Vec<DiagnosisRecord>>>,
    current_diagnosis: Arc<Mutex<Option<DiagnosisResult>>>,
}

impl RcaAgent {
    pub fn new(config: RcaConfig) -> Result<Self, String> {
        let mcp_client = McpClient::new(&config.mcp_server_url, &config.mcp_api_key)?;
        Ok(Self {
            config,
            mcp_client,
            history: Arc::new(Mutex::new(Vec::new())),
            current_diagnosis: Arc::new(Mutex::new(None)),
        })
    }

    /// Run a full diagnosis based on the trigger
    pub async fn diagnose(&self, request: DiagnosisRequest) -> Result<DiagnosisResult, String> {
        let diagnosis_id = Uuid::new_v4();
        let started_at = Utc::now();

        let mut result = DiagnosisResult {
            diagnosis_id,
            trigger: request.trigger.clone(),
            status: DiagnosisStatus::Running,
            root_cause: None,
            recommended_actions: Vec::new(),
            evidence: Vec::new(),
            confidence: 0.0,
            started_at,
            completed_at: None,
            llm_tokens_used: None,
        };

        // Store as current diagnosis
        {
            let mut current = self.current_diagnosis.lock().await;
            *current = Some(result.clone());
        }

        info!("Starting diagnosis {} for trigger: {:?}", diagnosis_id, request.trigger);

        // Collect evidence based on trigger type
        match &request.trigger {
            DiagnosisTrigger::AlertFired { alert_id, alert_type } => {
                self.collect_alert_evidence(&mut result, *alert_id, alert_type).await?;
            }
            DiagnosisTrigger::DeployFailed { deploy_id, commit_hash } => {
                self.collect_deploy_evidence(&mut result, *deploy_id, commit_hash).await?;
            }
            DiagnosisTrigger::Manual { description } => {
                self.collect_manual_evidence(&mut result, description, request.context).await?;
            }
            DiagnosisTrigger::Scheduled { schedule_name } => {
                self.collect_scheduled_evidence(&mut result, schedule_name).await?;
            }
        }

        // If we have a target server, focus evidence collection there
        if let Some(server_id) = request.target_server_id {
            self.collect_server_evidence(&mut result, server_id).await?;
        }

        // Analyze evidence with LLM to determine root cause and recommendations
        self.analyze_with_llm(&mut result).await?;

        result.status = DiagnosisStatus::Completed;
        result.completed_at = Some(Utc::now());

        // Store in history
        {
            let mut history = self.history.lock().await;
            history.push(DiagnosisRecord {
                id: result.diagnosis_id,
                trigger: request.trigger,
                status: result.status.clone(),
                root_cause: result.root_cause.clone(),
                recommended_actions: result.recommended_actions.clone(),
                evidence: result.evidence.clone(),
                confidence: result.confidence,
                started_at: result.started_at,
                completed_at: result.completed_at,
                feedback: None,
            });

            // Trim history if too large
            if history.len() > self.config.max_diagnosis_history {
                let excess = history.len() - self.config.max_diagnosis_history;
                history.drain(0..excess);
            }
        }

        // Clear current diagnosis
        {
            let mut current = self.current_diagnosis.lock().await;
            *current = None;
        }

        info!("Completed diagnosis {} with confidence {:.2}", result.diagnosis_id, result.confidence);
        Ok(result)
    }

    /// Collect evidence for alert-based diagnosis
    async fn collect_alert_evidence(
        &self,
        result: &mut DiagnosisResult,
        alert_id: Uuid,
        alert_type: &str,
    ) -> Result<(), String> {
        // Get alert details
        let alert_details = self.mcp_client
            .call_tool("get_alert_details", serde_json::json!({ "alert_id": alert_id }))
            .await?;

        result.evidence.push(EvidenceItem {
            source: EvidenceSource::Alert,
            timestamp: Utc::now(),
            summary: format!("Alert fired: {} ({})", alert_type, alert_id),
            details: alert_details,
            severity: EvidenceSeverity::Critical,
        });

        // Get related metrics for the alert's server
        if let Some(server_id) = alert_details.get("server_id").and_then(|v| v.as_str()) {
            if let Ok(server_uuid) = Uuid::parse_str(server_id) {
                self.collect_server_evidence(result, server_uuid).await?;
            }
        }

        // Get recent logs from affected services
        let logs = self.mcp_client
            .call_tool("get_system_logs", serde_json::json!({
                "log_type": "nginx_error",
                "lines": 200,
                "filter": alert_type
            }))
            .await?;

        result.evidence.push(EvidenceItem {
            source: EvidenceSource::Logs,
            timestamp: Utc::now(),
            summary: format!("Error logs related to {}", alert_type),
            details: logs,
            severity: EvidenceSeverity::Warning,
        });

        Ok(())
    }

    /// Collect evidence for failed deployment
    async fn collect_deploy_evidence(
        &self,
        result: &mut DiagnosisResult,
        deploy_id: Uuid,
        commit_hash: &str,
    ) -> Result<(), String> {
        // Get deploy details
        let deploy_details = self.mcp_client
            .call_tool("get_git_deploy", serde_json::json!({ "deploy_id": deploy_id }))
            .await?;

        result.evidence.push(EvidenceItem {
            source: EvidenceSource::DeployHistory,
            timestamp: Utc::now(),
            summary: format!("Deploy failed: {} (commit {})", deploy_id, commit_hash),
            details: deploy_details,
            severity: EvidenceSeverity::Critical,
        });

        // Get deploy logs
        let logs = self.mcp_client
            .call_tool("get_git_deploy_logs", serde_json::json!({ "deploy_id": deploy_id }))
            .await?;

        result.evidence.push(EvidenceItem {
            source: EvidenceSource::Logs,
            timestamp: Utc::now(),
            summary: "Deploy build/deploy logs",
            details: logs,
            severity: EvidenceSeverity::Warning,
        });

        // Get recent metrics for affected server
        if let Some(server_id) = deploy_details.get("server_id").and_then(|v| v.as_str()) {
            if let Ok(server_uuid) = Uuid::parse_str(server_id) {
                self.collect_server_evidence(result, server_uuid).await?;
            }
        }

        Ok(())
    }

    /// Collect evidence for manual diagnosis
    async fn collect_manual_evidence(
        &self,
        result: &mut DiagnosisResult,
        description: &str,
        context: Option<String>,
    ) -> Result<(), String> {
        result.evidence.push(EvidenceItem {
            source: EvidenceSource::McpTool,
            timestamp: Utc::now(),
            summary: format!("Manual diagnosis request: {}", description),
            details: serde_json::json!({
                "description": description,
                "context": context,
            }),
            severity: EvidenceSeverity::Info,
        });

        // Get fleet overview
        let fleet = self.mcp_client.call_tool("get_fleet_dashboard", serde_json::json!({})).await?;
        result.evidence.push(EvidenceItem {
            source: EvidenceSource::Metrics,
            timestamp: Utc::now(),
            summary: "Fleet dashboard snapshot",
            details: fleet,
            severity: EvidenceSeverity::Info,
        });

        // Get recent alerts
        let alerts = self.mcp_client
            .call_tool("list_alerts", serde_json::json!({ "status": "firing", "limit": 20 }))
            .await?;
        result.evidence.push(EvidenceItem {
            source: EvidenceSource::Alert,
            timestamp: Utc::now(),
            summary: "Currently firing alerts",
            details: alerts,
            severity: EvidenceSeverity::Warning,
        });

        Ok(())
    }

    /// Collect evidence for scheduled diagnosis
    async fn collect_scheduled_evidence(
        &self,
        result: &mut DiagnosisResult,
        schedule_name: &str,
    ) -> Result<(), String> {
        result.evidence.push(EvidenceItem {
            source: EvidenceSource::McpTool,
            timestamp: Utc::now(),
            summary: format!("Scheduled diagnosis: {}", schedule_name),
            details: serde_json::json!({ "schedule": schedule_name }),
            severity: EvidenceSeverity::Info,
        });

        // Get fleet health
        let health = self.mcp_client
            .call_tool("get_backup_orchestrator_health", serde_json::json!({}))
            .await?;
        result.evidence.push(EvidenceItem {
            source: EvidenceSource::McpTool,
            timestamp: Utc::now(),
            summary: "Backup orchestrator health",
            details: health,
            severity: EvidenceSeverity::Info,
        });

        let fleet = self.mcp_client.call_tool("get_fleet_dashboard", serde_json::json!({})).await?;
        result.evidence.push(EvidenceItem {
            source: EvidenceSource::Metrics,
            timestamp: Utc::now(),
            summary: "Fleet dashboard snapshot",
            details: fleet,
            severity: EvidenceSeverity::Info,
        });

        Ok(())
    }

    /// Collect evidence for a specific server
    async fn collect_server_evidence(
        &self,
        result: &mut DiagnosisResult,
        server_id: Uuid,
    ) -> Result<(), String> {
        // Get server metrics
        let metrics = self.mcp_client
            .call_tool("get_server_metrics", serde_json::json!({
                "server_id": server_id,
                "hours": 24
            }))
            .await?;

        result.evidence.push(EvidenceItem {
            source: EvidenceSource::Metrics,
            timestamp: Utc::now(),
            summary: format!("Server metrics (24h) for {}", server_id),
            details: metrics,
            severity: EvidenceSeverity::Info,
        });

        // Get container stats for apps on this server
        let apps = self.mcp_client.call_tool("list_apps", serde_json::json!({})).await?;
        let server_apps: Vec<_> = apps.as_array()
            .map(|arr| arr.iter()
                .filter(|a| a.get("server_id").and_then(|v| v.as_str()) == Some(&server_id.to_string()))
                .collect())
            .unwrap_or_default();

        for app in server_apps {
            if let Some(container_id) = app.get("container_id").and_then(|v| v.as_str()) {
                let stats = self.mcp_client
                    .call_tool("get_app_stats", serde_json::json!({ "container_id": container_id }))
                    .await?;
                result.evidence.push(EvidenceItem {
                    source: EvidenceSource::ContainerState,
                    timestamp: Utc::now(),
                    summary: format!("Container stats for {}", container_id),
                    details: stats,
                    severity: EvidenceSeverity::Info,
                });
            }
        }

        Ok(())
    }

    /// Analyze collected evidence with LLM to determine root cause
    async fn analyze_with_llm(&self, result: &mut DiagnosisResult) -> Result<(), String> {
        // Prepare evidence summary for LLM
        let evidence_summary = self.format_evidence_for_llm(&result.evidence);

        // Build prompt for LLM
        let prompt = self.build_diagnosis_prompt(&result.trigger, &evidence_summary);

        // Call LLM (this is a placeholder - would integrate with actual LLM provider)
        let (root_cause, actions, confidence, tokens) = self.call_llm(&prompt).await?;

        result.root_cause = Some(root_cause);
        result.recommended_actions = actions;
        result.confidence = confidence;
        result.llm_tokens_used = Some(tokens);

        Ok(())
    }

    /// Format evidence for LLM consumption
    fn format_evidence_for_llm(&self, evidence: &[EvidenceItem]) -> String {
        evidence.iter().map(|e| {
            format!(
                "[{}] {} - {}: {}",
                format!("{:?}", e.severity).to_uppercase(),
                format!("{:?}", e.source),
                e.summary,
                serde_json::to_string_pretty(&e.details).unwrap_or_default()
            )
        }).collect::<Vec<_>>().join("\n\n")
    }

    /// Build diagnosis prompt for LLM
    fn build_diagnosis_prompt(&self, trigger: &DiagnosisTrigger, evidence: &str) -> String {
        format!(
            r#"You are a senior DevOps engineer helping diagnose a ARSY server fleet issue.

TRIGGER: {:?}

EVIDENCE COLLECTED:
{}

Analyze this evidence and provide:
1. ROOT CAUSE: The most likely root cause in plain terms
2. RECOMMENDED ACTIONS: Specific, concrete steps to fix (in priority order)
3. CONFIDENCE: Your confidence level (0.0-1.0)

Be specific and actionable. Reference specific evidence items."#,
            trigger, evidence
        )
    }

    /// Call LLM for diagnosis (placeholder - integrate with actual provider)
    async fn call_llm(&self, prompt: &str) -> Result<(String, Vec<RecommendedAction>, f32, u32), String> {
        // This is a placeholder implementation
        // In production, this would call Anthropic, OpenAI, Gemini, or Grok API
        // based on self.config.llm_provider

        // For now, return a mock diagnosis based on simple pattern matching
        let (root_cause, actions, confidence) = if evidence.contains("disk") || evidence.contains("space") {
            (
                "Disk space exhaustion detected on one or more servers".to_string(),
                vec![
                    RecommendedAction {
                        action_type: ActionType::RunCommand,
                        description: "Check disk usage and clean up old backups/logs".to_string(),
                        command: Some("df -h && docker system prune -f".to_string()),
                        mcp_tool: None,
                        mcp_args: None,
                        priority: 1,
                        estimated_impact: "Immediate disk space recovery".to_string(),
                    },
                    RecommendedAction {
                        action_type: ActionType::CallMcpTool,
                        description: "Trigger backup cleanup via orchestrator".to_string(),
                        command: None,
                        mcp_tool: Some("trigger_backup_cleanup".to_string()),
                        mcp_args: Some(serde_json::json!({})),
                        priority: 2,
                        estimated_impact: "Remove old backups per retention policy".to_string(),
                    },
                ],
                0.85,
            )
        } else if evidence.contains("memory") || evidence.contains("OOM") {
            (
                "Memory pressure or OOM kills detected".to_string(),
                vec![
                    RecommendedAction {
                        action_type: ActionType::CallMcpTool,
                        description: "Check container memory limits and usage".to_string(),
                        command: None,
                        mcp_tool: Some("list_apps".to_string()),
                        mcp_args: Some(serde_json::json!({})),
                        priority: 1,
                        estimated_impact: "Identify memory-hungry containers".to_string(),
                    },
                    RecommendedAction {
                        action_type: ActionType::RunCommand,
                        description: "Restart OOM-killed containers".to_string(),
                        command: Some("docker ps -a | grep -i oom".to_string()),
                        mcp_tool: None,
                        mcp_args: None,
                        priority: 2,
                        estimated_impact: "Restore crashed services".to_string(),
                    },
                ],
                0.80,
            )
        } else if evidence.contains("deploy") || evidence.contains("build") {
            (
                "Git deploy build or deployment failure".to_string(),
                vec![
                    RecommendedAction {
                        action_type: ActionType::CallMcpTool,
                        description: "Get detailed deploy logs".to_string(),
                        command: None,
                        mcp_tool: Some("get_git_deploy_logs".to_string()),
                        mcp_args: Some(serde_json::json!({})),
                        priority: 1,
                        estimated_impact: "Identify exact build failure cause".to_string(),
                    },
                    RecommendedAction {
                        action_type: ActionType::RunCommand,
                        description: "Retry deploy with more verbose output".to_string(),
                        command: None,
                        mcp_tool: Some("trigger_git_deploy".to_string()),
                        mcp_args: Some(serde_json::json!({ "force": true })),
                        priority: 2,
                        estimated_impact: "Redeploy with fixes".to_string(),
                    },
                ],
                0.75,
            )
        } else {
            (
                "Unable to determine specific root cause from available evidence".to_string(),
                vec![
                    RecommendedAction {
                        action_type: ActionType::CheckLogs,
                        description: "Review system and application logs for anomalies".to_string(),
                        command: Some("journalctl -xe".to_string()),
                        mcp_tool: Some("get_system_logs".to_string()),
                        mcp_args: Some(serde_json::json!({ "lines": 500 })),
                        priority: 1,
                        estimated_impact: "Gather more diagnostic information".to_string(),
                    },
                    RecommendedAction {
                        action_type: ActionType::CheckMetrics,
                        description: "Check server metrics for anomalies".to_string(),
                        command: None,
                        mcp_tool: Some("get_fleet_dashboard".to_string()),
                        mcp_args: Some(serde_json::json!({})),
                        priority: 2,
                        estimated_impact: "Identify resource pressure or anomalies".to_string(),
                    },
                ],
                0.40,
            )
        };

        Ok((root_cause, actions, confidence, 1000)) // Mock token count
    }

    /// Get diagnosis history
    pub async fn get_history(&self) -> Vec<DiagnosisRecord> {
        self.history.lock().await.clone()
    }

    /// Get current diagnosis status
    pub async fn get_current(&self) -> Option<DiagnosisResult> {
        self.current_diagnosis.lock().await.clone()
    }

    /// Submit feedback for a diagnosis
    pub async fn submit_feedback(
        &self,
        diagnosis_id: Uuid,
        feedback: DiagnosisFeedback,
    ) -> Result<(), String> {
        let mut history = self.history.lock().await;
        if let Some(record) = history.iter_mut().find(|r| r.id == diagnosis_id) {
            record.feedback = Some(feedback);
            Ok(())
        } else {
            Err("Diagnosis not found".to_string())
        }
    }
}

/// MCP Client for calling ARSY MCP tools
pub struct McpClient {
    base_url: String,
    api_key: String,
    client: reqwest::Client,
}

impl McpClient {
    pub fn new(base_url: &str, api_key: &str) -> Result<Self, String> {
        Ok(Self {
            base_url: base_url.to_string(),
            api_key: api_key.to_string(),
            client: reqwest::Client::builder()
                .timeout(std::time::Duration::from_secs(30))
                .build()
                .map_err(|e| format!("Failed to create HTTP client: {e}"))?,
        })
    }

    /// Call an MCP tool
    pub async fn call_tool(&self, tool_name: &str, args: serde_json::Value) -> Result<serde_json::Value, String> {
        let url = format!("{}/mcp/tools/{}", self.base_url.trim_end_matches('/'), tool_name);

        let response = self.client
            .post(&url)
            .bearer_auth(&self.api_key)
            .json(&serde_json::json!({
                "jsonrpc": "2.0",
                "id": 1,
                "method": "tools/call",
                "params": {
                    "name": tool_name,
                    "arguments": args
                }
            }))
            .send()
            .await
            .map_err(|e| format!("MCP request failed: {e}"))?;

        let status = response.status();
        let body: serde_json::Value = response.json().await
            .map_err(|e| format!("Failed to parse MCP response: {e}"))?;

        if !status.is_success() {
            return Err(format!("MCP tool {} failed: {}", tool_name, body));
        }

        // Extract result from MCP response
        body.get("result")
            .cloned()
            .ok_or_else(|| format!("MCP tool {} returned no result", tool_name))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_evidence_severity_ordering() {
        assert!(EvidenceSeverity::Critical > EvidenceSeverity::Warning);
        assert!(EvidenceSeverity::Warning > EvidenceSeverity::Info);
    }

    #[test]
    fn test_diagnosis_status_transitions() {
        let statuses = vec![
            DiagnosisStatus::Pending,
            DiagnosisStatus::Running,
            DiagnosisStatus::Completed,
        ];
        assert_eq!(statuses.len(), 3);
    }
}