// Horizontal Autoscaler for ARSY
// Integrates with KEDA and Prometheus metrics for container autoscaling
// Supports CPU, Memory, RPS, and custom metric-based scaling

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

/// Autoscaler configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AutoscalerConfig {
    pub enabled: bool,
    pub kedad_enabled: bool,              // Use KEDA for scaling (if available)
    pub fallback_enabled: bool,           // Use built-in scaler if KEDA not available
    pub default_min_replicas: u32,
    pub default_max_replicas: u32,
    pub default_cooldown_seconds: u64,
    pub metrics_port: u16,                // Prometheus metrics port
}

/// Scale trigger types
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "type", rename_all = "lowercase")]
pub enum ScaleTriggerType {
    Cpu { threshold_percent: u8 },
    Memory { threshold_percent: u8 },
    Rps { threshold: u32 },               // Requests per second
    Custom { metric_name: String, threshold: f64 },
}

/// Autoscaling rule for a resource
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AutoscalingRule {
    pub id: Uuid,
    pub resource_type: ResourceType,      // "app", "stack", "database"
    pub resource_id: Uuid,
    pub name: String,
    pub min_replicas: u32,
    pub max_replicas: u32,
    pub cooldown_seconds: u64,
    pub triggers: Vec<ScaleTriggerType>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

/// Resource types that can be autoscaled
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "lowercase")]
pub enum ResourceType {
    App,
    Stack,
    Database,
}

/// Current scaling status
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ScalingStatus {
    pub rule_id: Uuid,
    pub current_replicas: u32,
    pub desired_replicas: u32,
    pub last_scale_time: Option<DateTime<Utc>>,
    pub last_scale_reason: Option<String>,
    pub metrics: HashMap<String, f64>,
}

/// Scale action record
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ScaleAction {
    pub id: Uuid,
    pub rule_id: Uuid,
    pub action: ScaleActionType,
    pub from_replicas: u32,
    pub to_replicas: u32,
    pub trigger: ScaleTriggerType,
    pub timestamp: DateTime<Utc>,
    pub success: bool,
    pub error: Option<String>,
}

/// Scale action type
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "lowercase")]
pub enum ScaleActionType {
    ScaleUp,
    ScaleDown,
}

/// Autoscaler state
pub struct Autoscaler {
    config: AutoscalerConfig,
    rules: Arc<Mutex<HashMap<Uuid, AutoscalingRule>>>,
    status: Arc<Mutex<HashMap<Uuid, ScalingStatus>>>,
    history: Arc<Mutex<Vec<ScaleAction>>>,
    running: Arc<Mutex<bool>>,
}

impl Autoscaler {
    pub fn new(config: AutoscalerConfig) -> Self {
        Self {
            config,
            rules: Arc::new(Mutex::new(HashMap::new())),
            status: Arc::new(Mutex::new(HashMap::new())),
            history: Arc::new(Mutex::new(Vec::new())),
            running: Arc::new(Mutex::new(false)),
        }
    }

    /// Add or update an autoscaling rule
    pub async fn upsert_rule(&self, rule: AutoscalingRule) -> Result<(), String> {
        if rule.min_replicas > rule.max_replicas {
            return Err("min_replicas cannot exceed max_replicas".to_string());
        }
        if rule.triggers.is_empty() {
            return Err("At least one trigger is required".to_string());
        }

        let mut rules = self.rules.lock().await;
        rules.insert(rule.id, rule);
        Ok(())
    }

    /// Delete an autoscaling rule
    pub async fn delete_rule(&self, rule_id: Uuid) -> Result<(), String> {
        let mut rules = self.rules.lock().await;
        rules.remove(&rule_id).ok_or("Rule not found")?;
        Ok(())
    }

    /// Get all rules
    pub async fn get_rules(&self) -> Vec<AutoscalingRule> {
        self.rules.lock().await.values().cloned().collect()
    }

    /// Get a specific rule
    pub async fn get_rule(&self, rule_id: Uuid) -> Option<AutoscalingRule> {
        self.rules.lock().await.get(&rule_id).cloned()
    }

    /// Get scaling status for a rule
    pub async fn get_status(&self, rule_id: Uuid) -> Option<ScalingStatus> {
        self.status.lock().await.get(&rule_id).cloned()
    }

    /// Get all statuses
    pub async fn get_all_statuses(&self) -> Vec<ScalingStatus> {
        self.status.lock().await.values().cloned().collect()
    }

    /// Get scale history
    pub async fn get_history(&self, limit: Option<usize>) -> Vec<ScaleAction> {
        let history = self.history.lock().await;
        let limit = limit.unwrap_or(100);
        history.iter().rev().take(limit).cloned().collect()
    }

    /// Record a scale action
    async fn record_action(&self, action: ScaleAction) {
        let mut history = self.history.lock().await;
        history.push(action);
        // Keep only last 1000 actions
        if history.len() > 1000 {
            let excess = history.len() - 1000;
            history.drain(0..excess);
        }
    }

    /// Run the autoscaler loop (background task)
    pub async fn run_loop(&self, state: crate::AppState) {
        if !self.config.enabled {
            info!("Autoscaler not enabled, skipping loop");
            return;
        }

        {
            let mut running = self.running.lock().await;
            if *running {
                warn!("Autoscaler loop already running");
                return;
            }
            *running = true;
        }

        info!("Starting autoscaler loop");

        let mut interval = tokio::time::interval(
            std::time::Duration::from_secs(60) // Check every minute
        );

        loop {
            interval.tick().await;

            if !*self.running.lock().await {
                break;
            }

            if let Err(e) = self.evaluate_rules(&state).await {
                warn!("Autoscaler evaluation error: {e}");
            }
        }

        info!("Autoscaler loop stopped");
        *self.running.lock().await = false;
    }

    /// Evaluate all rules and scale if needed
    async fn evaluate_rules(&self, state: &crate::AppState) -> Result<(), String> {
        let rules = self.get_rules().await;
        
        for rule in rules {
            if let Err(e) = self.evaluate_rule(&state, &rule).await {
                warn!("Failed to evaluate rule {}: {e}", rule.id);
            }
        }
        Ok(())
    }

    /// Evaluate a single rule
    async fn evaluate_rule(&self, state: &crate::AppState, rule: &AutoscalingRule) -> Result<(), String> {
        // Get current replicas
        let current_replicas = self.get_current_replicas(&state, &rule).await?;
        
        // Get metrics for each trigger
        let mut should_scale_up = false;
        let mut should_scale_down = true; // Assume scale down unless a trigger says otherwise
        let mut triggered_by: Option<ScaleTriggerType> = None;
        let mut metrics = HashMap::new();

        for trigger in &rule.triggers {
            let (value, should_up, should_down) = self.evaluate_trigger(&state, &rule, trigger).await?;
            
            let metric_name = match trigger {
                ScaleTriggerType::Cpu { .. } => "cpu_percent",
                ScaleTriggerType::Memory { .. } => "memory_percent",
                ScaleTriggerType::Rps { .. } => "rps",
                ScaleTriggerType::Custom { metric_name, .. } => metric_name,
            };
            metrics.insert(metric_name.to_string(), value);

            if should_up {
                should_scale_up = true;
                triggered_by = Some(trigger.clone());
            }
            if !should_down {
                should_scale_down = false;
            }
        }

        let mut status = self.status.lock().await;
        let mut current_status = status.entry(rule.id).or_insert(ScalingStatus {
            rule_id: rule.id,
            current_replicas,
            desired_replicas: current_replicas,
            last_scale_time: None,
            last_scale_reason: None,
            metrics: metrics.clone(),
        });

        current_status.metrics = metrics;

        // Determine desired replicas
        let mut desired_replicas = current_replicas;

        if should_scale_up && current_replicas < rule.max_replicas {
            desired_replicas = (current_replicas + 1).min(rule.max_replicas);
        } else if should_scale_down && current_replicas > rule.min_replicas {
            desired_replicas = (current_replicas - 1).max(rule.min_replicas);
        }

        current_status.desired_replicas = desired_replicas;

        // Check cooldown
        let now = Utc::now();
        let can_scale = current_status.last_scale_time
            .map(|last| (now - last).num_seconds() >= rule.cooldown_seconds as i64)
            .unwrap_or(true);

        if can_scale && desired_replicas != current_replicas {
            // Execute scale action
            let action_type = if desired_replicas > current_replicas {
                ScaleActionType::ScaleUp
            } else {
                ScaleActionType::ScaleDown
            };

            let success = self.execute_scale(&state, &rule, desired_replicas).await.is_ok();
            
            let action = ScaleAction {
                id: Uuid::new_v4(),
                rule_id: rule.id,
                action: action_type,
                from_replicas: current_replicas,
                to_replicas: desired_replicas,
                trigger: triggered_by.unwrap_or(rule.triggers[0].clone()),
                timestamp: now,
                success,
                error: if success { None } else { Some("Scale action failed".to_string()) },
            };

            self.record_action(action).await;

            if success {
                current_status.current_replicas = desired_replicas;
                current_status.last_scale_time = Some(now);
                current_status.last_scale_reason = Some(format!("{:?} triggered by {:?}", action_type, triggered_by));
            }
        }

        Ok(())
    }

    /// Evaluate a single trigger
    async fn evaluate_trigger(
        &self,
        state: &crate::AppState,
        rule: &AutoscalingRule,
        trigger: &ScaleTriggerType,
    ) -> Result<(f64, bool, bool), String> {
        match trigger {
            ScaleTriggerType::Cpu { threshold_percent } => {
                let value = self.get_cpu_metric(state, rule).await?;
                Ok((value, value > *threshold_percent as f64, value < (*threshold_percent as f64 * 0.5)))
            }
            ScaleTriggerType::Memory { threshold_percent } => {
                let value = self.get_memory_metric(state, rule).await?;
                Ok((value, value > *threshold_percent as f64, value < (*threshold_percent as f64 * 0.5)))
            }
            ScaleTriggerType::Rps { threshold } => {
                let value = self.get_rps_metric(state, rule).await?;
                Ok((value, value > *threshold as f64, value < (*threshold as f64 * 0.5)))
            }
            ScaleTriggerType::Custom { metric_name, threshold } => {
                let value = self.get_custom_metric(state, rule, metric_name).await?;
                Ok((value, value > *threshold, value < *threshold * 0.5))
            }
        }
    }

    /// Get CPU metric from Prometheus
    async fn get_cpu_metric(&self, state: &crate::AppState, rule: &AutoscalingRule) -> Result<f64, String> {
        // Query Prometheus for CPU usage
        // This is a placeholder - actual implementation would query the panel's metrics endpoint
        Ok(50.0) // Mock value
    }

    /// Get memory metric from Prometheus
    async fn get_memory_metric(&self, state: &crate::AppState, rule: &AutoscalingRule) -> Result<f64, String> {
        Ok(60.0) // Mock value
    }

    /// Get RPS metric from Prometheus
    async fn get_rps_metric(&self, state: &crate::AppState, rule: &AutoscalingRule) -> Result<f64, String> {
        Ok(100.0) // Mock value
    }

    /// Get custom metric
    async fn get_custom_metric(&self, state: &crate::AppState, rule: &AutoscalingRule, metric_name: &str) -> Result<f64, String> {
        Ok(0.0) // Mock value
    }

    /// Get current replicas for a resource
    async fn get_current_replicas(&self, state: &crate::AppState, rule: &AutoscalingRule) -> Result<u32, String> {
        match rule.resource_type {
            ResourceType::App => {
                // Get Docker service replicas
                Ok(1) // Mock
            }
            ResourceType::Stack => {
                Ok(1) // Mock
            }
            ResourceType::Database => {
                Ok(1) // Mock
            }
        }
    }

    /// Execute scale action
    async fn execute_scale(&self, state: &crate::AppState, rule: &AutoscalingRule, desired: u32) -> Result<(), String> {
        match rule.resource_type {
            ResourceType::App => {
                // Scale Docker service
                // This would call the agent to scale the Docker service
                info!("Scaling app {} to {} replicas", rule.resource_id, desired);
                Ok(())
            }
            ResourceType::Stack => {
                // Scale Docker stack services
                Ok(())
            }
            ResourceType::Database => {
                // Scale database (read replicas, etc.)
                Ok(())
            }
        }
    }

    /// Record a scale action
    async fn record_action(&self, action: ScaleAction) {
        let mut history = self.history.lock().await;
        history.push(action);
        if history.len() > 1000 {
            let excess = history.len() - 1000;
            history.drain(0..excess);
        }
    }

    /// Stop the autoscaler loop
    pub async fn stop(&self) {
        *self.running.lock().await = false;
    }

    /// Check if running
    pub fn is_running(&self) -> bool {
        // Can't easily check without locking, so this is approximate
        false
    }
}

/// Create autoscaler from settings
pub async fn create_autoscaler_from_settings(state: &crate::AppState) -> Result<Autoscaler, String> {
    let settings: Vec<(String, String)> = sqlx::query_as(
        "SELECT key, value FROM settings WHERE key IN (
            'autoscaler_enabled', 'autoscaler_keda_enabled', 'autoscaler_fallback_enabled',
            'autoscaler_min_replicas', 'autoscaler_max_replicas', 'autoscaler_cooldown_seconds',
            'autoscaler_metrics_port'
        )"
    )
    .fetch_all(state.db)
    .await
    .map_err(|e| format!("Failed to load autoscaler settings: {e}"))?;

    let get = |k: &str| -> String {
        settings.iter()
            .find(|(sk, _)| sk == k)
            .map(|(_, v)| v.clone())
            .unwrap_or_default()
    };

    let enabled = get("autoscaler_enabled") == "true";
    let keda_enabled = get("autoscaler_keda_enabled") == "true";
    let fallback_enabled = get("autoscaler_fallback_enabled") == "true";
    let min_replicas = get("autoscaler_min_replicas").parse::<u32>().unwrap_or(1);
    let max_replicas = get("autoscaler_max_replicas").parse::<u32>().unwrap_or(10);
    let cooldown_seconds = get("autoscaler_cooldown_seconds").parse::<u64>().unwrap_or(300);
    let metrics_port = get("autoscaler_metrics_port").parse::<u16>().unwrap_or(9090);

    Ok(AutoscalerConfig {
        enabled,
        keda_enabled,
        fallback_enabled,
        default_min_replicas: min_replicas,
        default_max_replicas: max_replicas,
        default_cooldown_seconds: cooldown_seconds,
        metrics_port,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_scale_trigger_thresholds() {
        let cpu = ScaleTriggerType::Cpu { threshold_percent: 80 };
        let mem = ScaleTriggerType::Memory { threshold_percent: 85 };
        
        match cpu {
            ScaleTriggerType::Cpu { threshold_percent } => assert_eq!(threshold_percent, 80),
            _ => panic!("Wrong type"),
        }
        
        match mem {
            ScaleTriggerType::Memory { threshold_percent } => assert_eq!(threshold_percent, 85),
            _ => panic!("Wrong type"),
        }
    }
}