// Policy-as-Code Admission Controller for ARSY
// Uses OPA/Rego for deploy-time policy evaluation
// Provides flexible, composable security and compliance policies

use crate::auth::AuthUser;
use crate::error::{internal_error, err, ApiError};
use crate::AppState;
use axum::extract::{Path, State};
use axum::Json;
use chrono::{DateTime, Utc};
use regorus::{Engine, Value};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::Mutex;
use tracing::{debug, info, warn};
use uuid::Uuid;

/// Policy Engine configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PolicyEngineConfig {
    pub enabled: bool,
    pub opa_url: Option<String>,              // External OPA server URL (if not embedded)
    pub bundled_policies_path: String,        // Path to built-in policies
    pub custom_policies_path: String,         // Path to user-defined policies
    pub default_decision: PolicyDecision,     // Default if no policy matches
    pub audit_mode: bool,                     // If true, log but don't enforce
}

/// Policy decision
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "lowercase")]
pub enum PolicyDecision {
    Allow,
    Deny,
    Warn,
}

/// Policy evaluation input
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PolicyInput {
    pub resource: PolicyResource,
    pub action: String,                    // "create", "update", "delete", "deploy"
    pub user: PolicyUser,
    pub context: HashMap<String, serde_json::Value>,
}

/// Resource being evaluated
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PolicyResource {
    pub kind: String,                      // "site", "database", "app", "stack", "deployment"
    pub name: String,
    pub namespace: Option<String>,
    pub labels: HashMap<String, String>,
    pub spec: serde_json::Value,
}

/// User making the request
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PolicyUser {
    pub id: Uuid,
    pub email: String,
    pub role: String,
    pub groups: Vec<String>,
}

/// Policy evaluation result
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PolicyResult {
    pub decision: PolicyDecision,
    pub matched_policies: Vec<MatchedPolicy>,
    pub warnings: Vec<String>,
    pub errors: Vec<String>,
    pub evaluation_time_ms: u64,
}

/// Matched policy details
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MatchedPolicy {
    pub id: String,
    pub name: String,
    pub category: PolicyCategory,
    pub severity: PolicySeverity,
    pub decision: PolicyDecision,
    pub message: String,
}

/// Policy categories
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "lowercase")]
pub enum PolicyCategory {
    Security,
    Compliance,
    ResourceManagement,
    CostOptimization,
    Operational,
    Custom,
}

/// Policy severity
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "lowercase")]
pub enum PolicySeverity {
    Critical,
    High,
    Medium,
    Low,
    Info,
}

/// Policy definition
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Policy {
    pub id: String,
    pub name: String,
    pub description: String,
    pub category: PolicyCategory,
    pub severity: PolicySeverity,
    pub rego: String,                      // Rego policy code
    pub enabled: bool,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

/// Policy evaluation request
#[derive(Debug, Deserialize)]
pub struct EvaluateRequest {
    pub input: PolicyInput,
}

/// Policy evaluation response
#[derive(Debug, Serialize)]
pub struct EvaluateResponse {
    pub result: PolicyResult,
    pub input: PolicyInput,
}

/// Policy engine state
pub struct PolicyEngine {
    config: PolicyEngineConfig,
    policies: Arc<Mutex<HashMap<String, Policy>>>,
    opa_client: Option<reqwest::Client>,
}

impl PolicyEngine {
    pub fn new(config: PolicyEngineConfig) -> Self {
        let opa_client = config.opa_url.as_ref().map(|url| {
            reqwest::Client::builder()
                .timeout(std::time::Duration::from_secs(10))
                .build()
                .unwrap_or_else(|_| reqwest::Client::new())
        });

        Self {
            config,
            policies: Arc::new(Mutex::new(HashMap::new())),
            opa_client,
        }
    }

    /// Load built-in policies
    pub async fn load_builtin_policies(&self) -> Result<usize, String> {
        let mut policies = self.policies.lock().await;
        let mut count = 0;

        // Built-in security policies
        let builtin_policies = vec![
            self.create_policy(
                "security.no-privileged-containers",
                "Disallow privileged containers",
                "Prevent containers from running with privileged access",
                PolicyCategory::Security,
                PolicySeverity::Critical,
                r#"
package arsy.security

deny[msg] {
    input.resource.kind == "app"
    input.resource.spec.privileged == true
    msg := "Privileged containers are not allowed"
}

deny[msg] {
    input.resource.kind == "stack"
    service := input.resource.spec.services[_]
    service.privileged == true
    msg := sprintf("Service %s in stack %s runs privileged", [service.name, input.resource.name])
}
"#,
            ),
            self.create_policy(
                "security.no-host-network",
                "Disallow host network mode",
                "Prevent containers from using host network",
                PolicyCategory::Security,
                PolicySeverity::High,
                r#"
package arsy.security

deny[msg] {
    input.resource.kind == "app"
    input.resource.spec.network_mode == "host"
    msg := "Host network mode is not allowed"
}

deny[msg] {
    input.resource.kind == "stack"
    service := input.resource.spec.services[_]
    service.network_mode == "host"
    msg := sprintf("Service %s in stack %s uses host network", [service.name, input.resource.name])
}
"#,
            ),
            self.create_policy(
                "security.no-host-pid-ipc",
                "Disallow host PID/IPC namespaces",
                "Prevent containers from sharing host PID/IPC namespaces",
                PolicyCategory::Security,
                PolicySeverity::High,
                r#"
package arsy.security

deny[msg] {
    input.resource.kind == "app"
    input.resource.spec.pid_mode == "host"
    msg := "Host PID namespace is not allowed"
}

deny[msg] {
    input.resource.kind == "app"
    input.resource.spec.ipc_mode == "host"
    msg := "Host IPC namespace is not allowed"
}

deny[msg] {
    input.resource.kind == "stack"
    service := input.resource.spec.services[_]
    service.pid_mode == "host"
    msg := sprintf("Service %s in stack %s uses host PID namespace", [service.name, input.resource.name])
}

deny[msg] {
    input.resource.kind == "stack"
    service := input.resource.spec.services[_]
    service.ipc_mode == "host"
    msg := sprintf("Service %s in stack %s uses host IPC namespace", [service.name, input.resource.name])
}
"#,
            ),
            self.create_policy(
                "security.no-dangerous-capabilities",
                "Disallow dangerous Linux capabilities",
                "Prevent containers from adding dangerous capabilities",
                PolicyCategory::Security,
                PolicySeverity::Critical,
                r#"
package arsy.security

dangerous_capabilities := {"SYS_ADMIN", "SYS_PTRACE", "NET_ADMIN", "ALL", "NET_RAW", "SYS_RAWIO", "SYS_MODULE", "DAC_OVERRIDE"}

deny[msg] {
    input.resource.kind == "app"
    cap := input.resource.spec.cap_add[_]
    dangerous_capabilities[cap]
    msg := sprintf("Dangerous capability %s is not allowed", [cap])
}

deny[msg] {
    input.resource.kind == "stack"
    service := input.resource.spec.services[_]
    cap := service.cap_add[_]
    dangerous_capabilities[cap]
    msg := sprintf("Service %s in stack %s adds dangerous capability %s", [service.name, input.resource.name, cap])
}
"#,
            ),
            self.create_policy(
                "security.no-host-path-volumes",
                "Disallow host path volumes",
                "Prevent containers from mounting sensitive host paths",
                PolicyCategory::Security,
                PolicySeverity::Critical,
                r#"
package arsy.security

sensitive_paths := {"/etc", "/root", "/home", "/var/run/docker.sock", "/var/run", "/proc", "/sys", "/dev"}

deny[msg] {
    input.resource.kind == "app"
    volume := input.resource.spec.volumes[_]
    volume.type == "hostPath"
    sensitive_paths[volume.host_path.path]
    msg := sprintf("Host path volume %s mounts sensitive path %s", [volume.name, volume.host_path.path])
}

deny[msg] {
    input.resource.kind == "stack"
    service := input.resource.spec.services[_]
    volume := service.volumes[_]
    volume.type == "hostPath"
    sensitive_paths[volume.host_path.path]
    msg := sprintf("Service %s in stack %s mounts sensitive path %s", [service.name, input.resource.name, volume.host_path.path])
}
"#,
            ),
            self.create_policy(
                "resource.limits.required",
                "Require resource limits",
                "All containers must have CPU and memory limits defined",
                PolicyCategory::ResourceManagement,
                PolicySeverity::Medium,
                r#"
package arsy.resource

deny[msg] {
    input.resource.kind == "app"
    not input.resource.spec.resources.limits.cpu
    msg := "CPU limit is required for all containers"
}

deny[msg] {
    input.resource.kind == "app"
    not input.resource.spec.resources.limits.memory
    msg := "Memory limit is required for all containers"
}

deny[msg] {
    input.resource.kind == "stack"
    service := input.resource.spec.services[_]
    not service.resources.limits.cpu
    msg := sprintf("Service %s in stack %s missing CPU limit", [service.name, input.resource.name])
}

deny[msg] {
    input.resource.kind == "stack"
    service := input.resource.spec.services[_]
    not service.resources.limits.memory
    msg := sprintf("Service %s in stack %s missing memory limit", [service.name, input.resource.name])
}
"#,
            ),
            self.create_policy(
                "compliance.image-signing.required",
                "Require signed container images",
                "All container images must be signed with cosign",
                PolicyCategory::Compliance,
                PolicySeverity::High,
                r#"
package arsy.compliance

deny[msg] {
    input.resource.kind == "app"
    not input.resource.spec.image_signed
    msg := "Container image must be signed with cosign"
}

deny[msg] {
    input.resource.kind == "stack"
    service := input.resource.spec.services[_]
    not service.image_signed
    msg := sprintf("Service %s in stack %s uses unsigned image", [service.name, input.resource.name])
}
"#,
            ),
            self.create_policy(
                "compliance.base-image.allowed",
                "Only allow approved base images",
                "Container images must use approved base images",
                PolicyCategory::Compliance,
                PolicySeverity::High,
                r#"
package arsy.compliance

allowed_base_images := {
    "docker.io/library/nginx:",
    "docker.io/library/postgres:",
    "docker.io/library/mysql:",
    "docker.io/library/redis:",
    "docker.io/library/node:",
    "docker.io/library/python:",
    "ghcr.io/",
    "registry.example.com/",
}

deny[msg] {
    input.resource.kind == "app"
    image := input.resource.spec.image
    not any(allowed in allowed_base_images: image.starts_with(allowed))
    msg := sprintf("Base image %s is not in allowed list", [image])
}

deny[msg] {
    input.resource.kind == "stack"
    service := input.resource.spec.services[_]
    image := service.image
    not any(allowed in allowed_base_images: image.starts_with(allowed))
    msg := sprintf("Service %s in stack %s uses unapproved base image %s", [service.name, input.resource.name, image])
}
"#,
            ),
            self.create_policy(
                "cost.optimization.no-unlimited-resources",
                "Prevent unlimited resource requests",
                "Containers should not request unlimited CPU/memory",
                PolicyCategory::CostOptimization,
                PolicySeverity::Medium,
                r#"
package arsy.cost

deny[msg] {
    input.resource.kind == "app"
    input.resource.spec.resources.requests.cpu == "0"
    msg := "CPU request cannot be unlimited (0)"
}

deny[msg] {
    input.resource.kind == "app"
    input.resource.spec.resources.requests.memory == "0"
    msg := "Memory request cannot be unlimited (0)"
}

deny[msg] {
    input.resource.kind == "app"
    input.resource.spec.resources.limits.cpu == "0"
    msg := "CPU limit cannot be unlimited (0)"
}

deny[msg] {
    input.resource.kind == "app"
    input.resource.spec.resources.limits.memory == "0"
    msg := "Memory limit cannot be unlimited (0)"
}
"#,
            ),
        ];

        for policy in builtin_policies {
            policies.insert(policy.id.clone(), policy);
            count += 1;
        }

        Ok(count)
    }

    fn create_policy(
        &self,
        id: &str,
        name: &str,
        description: &str,
        category: PolicyCategory,
        severity: PolicySeverity,
        rego: &str,
    ) -> Policy {
        Policy {
            id: id.to_string(),
            name: name.to_string(),
            description: description.to_string(),
            category,
            severity,
            rego: rego.to_string(),
            enabled: true,
            created_at: Utc::now(),
            updated_at: Utc::now(),
        }
    }

    /// Evaluate a policy input against all policies
    pub async fn evaluate(&self, input: PolicyInput) -> PolicyResult {
        let start = std::time::Instant::now();
        let policies = self.policies.lock().await;
        let mut result = PolicyResult {
            decision: self.config.default_decision.clone(),
            matched_policies: Vec::new(),
            warnings: Vec::new(),
            errors: Vec::new(),
            evaluation_time_ms: 0,
        };

        // If using external OPA, delegate to it
        if let Some(opa_url) = &self.config.opa_url {
            if let Ok(external_result) = self.evaluate_external(opa_url, &input).await {
                return external_result;
            }
            result.warnings.push("External OPA evaluation failed, falling back to embedded".to_string());
        }

        // Evaluate each enabled policy
        for policy in policies.values() {
            if !policy.enabled {
                continue;
            }

            match self.evaluate_policy(&policy, &input).await {
                Ok(Some(decision)) => {
                    let matched = MatchedPolicy {
                        id: policy.id.clone(),
                        name: policy.name.clone(),
                        category: policy.category.clone(),
                        severity: policy.severity.clone(),
                        decision: decision.clone(),
                        message: format!("Policy %s matched", policy.name),
                    };
                    result.matched_policies.push(matched);

                    // Update overall decision based on severity
                    match decision {
                        PolicyDecision::Deny => {
                            if policy.severity == PolicySeverity::Critical || policy.severity == PolicySeverity::High {
                                result.decision = PolicyDecision::Deny;
                            } else if result.decision != PolicyDecision::Deny {
                                result.decision = PolicyDecision::Deny;
                            }
                        }
                        PolicyDecision::Warn => {
                            if result.decision == PolicyDecision::Allow {
                                result.decision = PolicyDecision::Warn;
                            }
                        }
                        PolicyDecision::Allow => {
                            // Allow doesn't override deny/warn
                        }
                    }
                Ok(None) => {
                    // Policy didn't match (no decision)
                }
                Err(e) => {
                    result.errors.push(format!("Policy {} evaluation error: {}", policy.id, e));
                }
            }
        }

        result.evaluation_time_ms = start.elapsed().as_millis() as u64;
        result
    }

    async fn evaluate_policy(&self, policy: &Policy, input: &PolicyInput) -> Result<Option<PolicyDecision>, String> {
        // Create a new engine
        let mut engine = Engine::new();

        // Add the policy module
        engine
            .add_policy(policy.rego)
            .map_err(|e| format!("Failed to add policy {}: {}", policy.id, e))?;

        // Add the query module for decision extraction
        let query_rego = r#"
package arsy.query

decision = "allow" {
    not deny[_]
    not warn[_]
}

decision = "warn" {
    warn[_]
    not deny[_]
}

decision = "deny" {
    deny[_]
}
"#;
        engine
            .add_policy(query_rego)
            .map_err(|e| format!("Failed to add query module: {}", e))?;

        // Prepare the input for evaluation
        let input_json = serde_json::to_value(input).map_err(|e| format!("Failed to serialize input: {}", e))?;

        // Convert serde_json::Value to regorus::Value
        let input_value = json_value_to_regorus(input_json)?;

        // Evaluate the query
        let results = engine
            .eval_query("data.arsy.query.decision", input_value)
            .map_err(|e| format!("Query evaluation failed: {}", e))?;

        // Extract the decision from the results
        for result in results {
            if let Some(Value::String(decision)) = result.get("decision") {
                return Ok(Some(match decision.as_str() {
                    "deny" => PolicyDecision::Deny,
                    "warn" => PolicyDecision::Warn,
                    _ => PolicyDecision::Allow,
                }));
            }
        }

        Ok(None)
    }

fn json_value_to_regorus(v: serde_json::Value) -> Result<Value, String> {
    match v {
        serde_json::Value::Null => Ok(Value::Null),
        serde_json::Value::Bool(b) => Ok(Value::Bool(b)),
        serde_json::Value::Number(n) => {
            if let Some(i) = n.as_i64() {
                Ok(Value::Int(i))
            } else if let Some(f) = n.as_f64() {
                Ok(Value::Float(f))
            } else {
                Ok(Value::Null)
            }
        }
        serde_json::Value::String(s) => Ok(Value::String(s)),
        serde_json::Value::Array(arr) => {
            let mut vec = Vec::new();
            for v in arr {
                vec.push(json_value_to_regorus(v)?);
            }
            Ok(Value::Array(vec))
        }
        serde_json::Value::Object(obj) => {
            let mut map = std::collections::BTreeMap::new();
            for (k, v) in obj {
                map.insert(k, json_value_to_regorus(v)?);
            }
            Ok(Value::Object(map))
        }
    }

    async fn evaluate_external(&self, opa_url: &str, input: &PolicyInput) -> Result<PolicyResult, String> {
        if let Some(client) = &self.opa_client {
            let response = client
                .post(&format!("{}/v1/data/arsy/authz", opa_url.trim_end_matches('/')))
                .json(&serde_json::json!({"input": input}))
                .send()
                .await
                .map_err(|e| format!("External OPA request failed: {e}"))?;

            let status = response.status();
            let body: serde_json::Value = response.json().await
                .map_err(|e| format!("Failed to parse OPA response: {e}"))?;

            if !status.is_success() {
                return Err(format!("External OPA returned {}: {}", status, body));
            }

            // Parse OPA response
            let result = body.get("result").cloned().unwrap_or(serde_json::json!({}));
            let decision = result.get("decision").and_then(|d| d.as_str()).unwrap_or("allow");
            
            Ok(PolicyResult {
                decision: match decision {
                    "deny" => PolicyDecision::Deny,
                    "warn" => PolicyDecision::Warn,
                    _ => PolicyDecision::Allow,
                },
                matched_policies: Vec::new(),
                warnings: Vec::new(),
                errors: Vec::new(),
                evaluation_time_ms: 0,
            })
        } else {
            Err("No OPA client".to_string())
        }
    }

    /// Add or update a custom policy
    pub async fn upsert_policy(&self, policy: Policy) -> Result<(), String> {
        if policy.rego.is_empty() {
            return Err("Policy rego cannot be empty".to_string());
        }
        if policy.id.is_empty() {
            return Err("Policy ID cannot be empty".to_string());
        }

        let mut policies = self.policies.lock().await;
        policies.insert(policy.id.clone(), policy);
        Ok(())
    }

    /// Delete a policy
    pub async fn delete_policy(&self, policy_id: &str) -> Result<(), String> {
        let mut policies = self.policies.lock().await;
        policies.remove(policy_id).ok_or("Policy not found")?;
        Ok(())
    }

    /// Get all policies
    pub async fn get_policies(&self) -> Vec<Policy> {
        self.policies.lock().await.values().cloned().collect()
    }

    /// Get a specific policy
    pub async fn get_policy(&self, policy_id: &str) -> Option<Policy> {
        self.policies.lock().await.get(policy_id).cloned()
    }
}

/// Create policy engine from settings
pub async fn create_policy_engine_from_settings(state: &crate::AppState) -> Result<PolicyEngine, String> {
    let settings: Vec<(String, String)> = sqlx::query_as(
        "SELECT key, value FROM settings WHERE key IN (
            'policy_engine_enabled', 'policy_engine_opa_url',
            'policy_engine_bundled_policies_path', 'policy_engine_custom_policies_path',
            'policy_engine_default_decision', 'policy_engine_audit_mode'
        )"
    )
    .fetch_all(state.db)
    .await
    .map_err(|e| format!("Failed to load policy engine settings: {e}"))?;

    let get = |k: &str| -> String {
        settings.iter()
            .find(|(sk, _)| sk == k)
            .map(|(_, v)| v.clone())
            .unwrap_or_default()
    };

    let enabled = get("policy_engine_enabled") == "true";
    let opa_url = if get("policy_engine_opa_url").is_empty() { None } else { Some(get("policy_engine_opa_url")) };
    let bundled_policies_path = get("policy_engine_bundled_policies_path").unwrap_or_else(|| "/etc/arsy/policies/bundled".to_string());
    let custom_policies_path = get("policy_engine_custom_policies_path").unwrap_or_else(|| "/etc/arsy/policies/custom".to_string());
    let default_decision = match get("policy_engine_default_decision").as_str() {
        "deny" => PolicyDecision::Deny,
        "warn" => PolicyDecision::Warn,
        _ => PolicyDecision::Allow,
    };
    let audit_mode = get("policy_engine_audit_mode") == "true";

    Ok(PolicyEngineConfig {
        enabled,
        opa_url,
        bundled_policies_path,
        custom_policies_path,
        default_decision,
        audit_mode,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_policy_engine_creation() {
        let config = PolicyEngineConfig {
            enabled: true,
            opa_url: None,
            bundled_policies_path: "/tmp/policies".to_string(),
            custom_policies_path: "/tmp/custom".to_string(),
            default_decision: PolicyDecision::Allow,
            audit_mode: false,
        };

        let engine = PolicyEngine::new(config);
        let count = engine.load_builtin_policies().await.unwrap();
        assert!(count > 0);
    }

    #[test]
    fn test_policy_decision_ordering() {
        assert_eq!(PolicyDecision::Deny, PolicyDecision::Deny);
        assert_ne!(PolicyDecision::Allow, PolicyDecision::Deny);
    }
}