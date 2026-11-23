// WAL-G integration for PostgreSQL Point-in-Time Recovery (PITR)
// WAL-G is a backup tool for PostgreSQL that supports WAL archiving to S3/GCS/Azure

use crate::safe_cmd::safe_command;
use std::path::PathBuf;
use std::time::Duration;

const WALG_DIR: &str = "/var/lib/arsy/wal-g";
const WALG_BIN: &str = "/var/lib/arsy/wal-g/wal-g";
const WALG_VERSION: &str = "v2.0.1"; // Pinned version for reproducibility

/// Check if WAL-G binary is installed
pub async fn is_installed() -> bool {
    tokio::fs::metadata(WALG_BIN).await.is_ok()
}

/// Install WAL-G binary
pub async fn install_wal_g() -> Result<(), String> {
    if is_installed().await {
        return Ok(());
    }

    tokio::fs::create_dir_all(WALG_DIR)
        .await
        .map_err(|e| format!("create {WALG_DIR}: {e}"))?;

    // Download WAL-G binary from GitHub releases
    let url = format!(
        "https://github.com/wal-g/wal-g/releases/download/{WALG_VERSION}/wal-g-pg-ubuntu-20.04-amd64.tar.gz"
    );

    let output = tokio::time::timeout(
        Duration::from_secs(180),
        safe_command("sh")
            .args(["-c", &format!(
                "cd {WALG_DIR} && curl -sSfL '{url}' | tar -xz && chmod +x wal-g"
            )])
            .kill_on_drop(true)
            .output(),
    )
    .await
    .map_err(|_| "WAL-G download timed out (180s)".to_string())?
    .map_err(|e| format!("WAL-G download failed: {e}"))?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(format!("WAL-G download failed: {stderr}"));
    }

    Ok(())
}

/// Uninstall WAL-G binary
pub async fn uninstall_wal_g() -> Result<(), String> {
    let _ = tokio::fs::remove_file(WALG_BIN).await;
    Ok(())
}

/// WAL-G configuration for a PostgreSQL database
#[derive(Debug, Clone)]
pub struct WalGConfig {
    pub database_name: String,
    pub s3_bucket: String,
    pub s3_region: String,
    pub s3_endpoint: Option<String>,
    pub aws_access_key_id: String,
    pub aws_secret_access_key: String,
    pub pg_data_dir: String, // Path to PostgreSQL data directory in container
    pub wal_level: String,   // Should be "replica" or "logical"
    pub archive_mode: String, // Should be "on"
    pub archive_command: String, // WAL-G push command
}

/// Generate WAL-G environment variables for the container
pub fn wal_g_env(config: &WalGConfig) -> Vec<(String, String)> {
    let mut env = vec![
        ("WALE_S3_PREFIX".to_string(), format!("s3://{}", config.s3_bucket)),
        ("AWS_REGION".to_string(), config.s3_region.clone()),
        ("AWS_ACCESS_KEY_ID".to_string(), config.aws_access_key_id.clone()),
        ("AWS_SECRET_ACCESS_KEY".to_string(), config.aws_secret_access_key.clone()),
    ];

    if let Some(endpoint) = &config.s3_endpoint {
        env.push(("AWS_ENDPOINT".to_string(), endpoint.clone()));
        env.push(("AWS_S3_FORCE_PATH_STYLE".to_string(), "true".to_string()));
    }

    env
}

/// Generate WAL-G environment variables and archive_command for PostgreSQL container
/// Returns (archive_command, env_vars) to be used when creating the container
pub fn wal_g_container_config(config: &WalGConfig) -> (String, Vec<(String, String)>) {
    let env_vars = wal_g_env(config);
    let archive_command = format!("wal-g wal-push %p");
    (archive_command, env_vars)
}

/// Generate PostgreSQL configuration for WAL archiving
pub fn pg_wal_archive_config(config: &WalGConfig) -> String {
    let env_vars = wal_g_env(config);
    let env_str = env_vars
        .iter()
        .map(|(k, v)| format!("{}={}", k, v))
        .collect::<Vec<_>>()
        .join(" ");

    format!(
        r#"# WAL-G archiving configuration
wal_level = {}
archive_mode = {}
archive_command = '{} wal-g wal-push %p'
archive_timeout = 60
max_wal_senders = 10
wal_keep_segments = 32
"#,
        config.wal_level, config.archive_mode, env_str
    )
}

/// Run WAL-G backup-push for a database
pub async fn backup_push(config: &WalGConfig, container_name: &str) -> Result<String, String> {
    let env_vars = wal_g_env(config);
    let mut cmd = safe_command("docker");
    cmd.args(["exec", "-i", container_name, "sh", "-c", "wal-g backup-push /var/lib/postgresql/data"]);

    for (k, v) in env_vars {
        cmd.env(k, v);
    }

    let output = tokio::time::timeout(
        Duration::from_secs(3600), // 1 hour for full backup
        cmd.kill_on_drop(true).output(),
    )
    .await
    .map_err(|_| "WAL-G backup-push timed out (3600s)".to_string())?
    .map_err(|e| format!("WAL-G backup-push failed to start: {e}"))?;

    let stdout = String::from_utf8_lossy(&output.stdout).to_string();
    let stderr = String::from_utf8_lossy(&output.stderr).to_string();
    let full_output = format!("{stdout}\n{stderr}");

    if !output.status.success() {
        return Err(format!("WAL-G backup-push failed:\n{full_output}"));
    }

    Ok(full_output)
}

/// Run WAL-G backup-fetch for PITR restore
pub async fn backup_fetch(config: &WalGConfig, container_name: &str, target_time: Option<&str>) -> Result<String, String> {
    let env_vars = wal_g_env(config);
    let mut cmd = safe_command("docker");
    
    let fetch_cmd = if let Some(time) = target_time {
        format!("wal-g backup-fetch /var/lib/postgresql/data LATEST --until '{}'", time)
    } else {
        "wal-g backup-fetch /var/lib/postgresql/data LATEST".to_string()
    };

    let mut cmd = safe_command("docker");
    cmd.args(["exec", "-i", container_name, "sh", "-c", &fetch_cmd]);

    for (k, v) in env_vars {
        cmd.env(k, v);
    }

    let output = tokio::time::timeout(
        Duration::from_secs(3600),
        cmd.kill_on_drop(true).output(),
    )
    .await
    .map_err(|_| "WAL-G backup-fetch timed out (3600s)".to_string())?
    .map_err(|e| format!("WAL-G backup-fetch failed to start: {e}"))?;

    let stdout = String::from_utf8_lossy(&output.stdout).to_string();
    let stderr = String::from_utf8_lossy(&output.stderr).to_string();
    let full_output = format!("{stdout}\n{stderr}");

    if !output.status.success() {
        return Err(format!("WAL-G backup-fetch failed:\n{full_output}"));
    }

    Ok(full_output)
}

/// List available WAL-G backups
pub async fn backup_list(config: &WalGConfig, container_name: &str) -> Result<String, String> {
    let env_vars = wal_g_env(config);
    let mut cmd = safe_command("docker");
    cmd.args(["exec", "-i", container_name, "wal-g", "backup-list"]);

    for (k, v) in env_vars {
        cmd.env(k, v);
    }

    let output = tokio::time::timeout(
        Duration::from_secs(60),
        cmd.kill_on_drop(true).output(),
    )
    .await
    .map_err(|_| "WAL-G backup-list timed out (60s)".to_string())?
    .map_err(|e| format!("WAL-G backup-list failed to start: {e}"))?;

    let stdout = String::from_utf8_lossy(&output.stdout).to_string();
    let stderr = String::from_utf8_lossy(&output.stderr).to_string();

    if !output.status.success() {
        return Err(format!("WAL-G backup-list failed: {}", stderr));
    }

    Ok(stdout)
}

/// Delete old WAL-G backups (retention)
pub async fn delete_retain(config: &WalGConfig, container_name: &str, retain_count: u32) -> Result<String, String> {
    let env_vars = wal_g_env(config);
    let mut cmd = safe_command("docker");
    cmd.args(["exec", "-i", container_name, "wal-g", "delete", "retain", retain_count.to_string(), "--confirm"]);

    for (k, v) in env_vars {
        cmd.env(k, v);
    }

    let output = tokio::time::timeout(
        Duration::from_secs(300),
        cmd.kill_on_drop(true).output(),
    )
    .await
    .map_err(|_| "WAL-G delete retain timed out (300s)".to_string())?
    .map_err(|e| format!("WAL-G delete retain failed to start: {e}"))?;

    let stdout = String::from_utf8_lossy(&output.stdout).to_string();
    let stderr = String::from_utf8_lossy(&output.stderr).to_string();

    if !output.status.success() {
        return Err(format!("WAL-G delete retain failed: {}", stderr));
    }

    Ok(stdout)
}

/// Start WAL-G wal-push as a continuous background process for WAL archiving
/// This runs `wal-g wal-push %p` continuously, pushing WAL files to S3 as they are generated
pub async fn start_wal_push(config: &WalGConfig, container_name: &str) -> Result<(), String> {
    let env_vars = wal_g_env(config);
    let mut cmd = safe_command("docker");
    cmd.args([
        "exec",
        "-d", // detached mode
        container_name,
        "sh",
        "-c",
        "exec wal-g wal-push %p",
    ]);

    for (k, v) in env_vars {
        cmd.env(k, v);
    }

    // Start the process in background
    let mut child = cmd
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .map_err(|e| format!("Failed to start wal-g wal-push: {e}"))?;

    // Don't wait for the process - it runs in background
    // The child process will be adopted by init (PID 1) if the parent exits
    std::mem::forget(child);

    Ok(())
}

/// Verify WAL-G backup integrity
pub async fn backup_verify(config: &WalGConfig, container_name: &str, backup_name: &str) -> Result<String, String> {
    let env_vars = wal_g_env(config);
    let mut cmd = safe_command("docker");
    cmd.args(["exec", "-i", container_name, "wal-g", "verify", backup_name]);

    for (k, v) in env_vars {
        cmd.env(k, v);
    }

    let output = tokio::time::timeout(
        Duration::from_secs(3600),
        cmd.kill_on_drop(true).output(),
    )
    .await
    .map_err(|_| "WAL-G verify timed out (3600s)".to_string())?
    .map_err(|e| format!("WAL-G verify failed to start: {e}"))?;

    let stdout = String::from_utf8_lossy(&output.stdout).to_string();
    let stderr = String::from_utf8_lossy(&output.stderr).to_string();

    if !output.status.success() {
        return Err(format!("WAL-G verify failed: {}", stderr));
    }

    Ok(stdout)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_wal_g_env() {
        let config = WalGConfig {
            database_name: "testdb".to_string(),
            s3_bucket: "my-bucket".to_string(),
            s3_region: "us-east-1".to_string(),
            s3_endpoint: None,
            aws_access_key_id: "AKIA...".to_string(),
            aws_secret_access_key: "secret".to_string(),
            pg_data_dir: "/var/lib/postgresql/data".to_string(),
            wal_level: "replica".to_string(),
            archive_mode: "on".to_string(),
            archive_command: "wal-g wal-push %p".to_string(),
        };

        let env = wal_g_env(&config);
        assert!(env.iter().any(|(k, v)| k == "WALE_S3_PREFIX" && v == "s3://my-bucket"));
        assert!(env.iter().any(|(k, v)| k == "AWS_REGION" && v == "us-east-1"));
    }

    #[test]
    fn test_pg_wal_archive_config() {
        let config = WalGConfig {
            database_name: "testdb".to_string(),
            s3_bucket: "my-bucket".to_string(),
            s3_region: "us-east-1".to_string(),
            s3_endpoint: None,
            aws_access_key_id: "AKIA...".to_string(),
            aws_secret_access_key: "secret".to_string(),
            pg_data_dir: "/var/lib/postgresql/data".to_string(),
            wal_level: "replica".to_string(),
            archive_mode: "on".to_string(),
            archive_command: "wal-g wal-push %p".to_string(),
        };

        let config_str = pg_wal_archive_config(&config);
        assert!(config_str.contains("wal_level = replica"));
        assert!(config_str.contains("archive_mode = on"));
        assert!(config_str.contains("archive_command = '"));
        assert!(config_str.contains("wal-g wal-push %p"));
    }
}