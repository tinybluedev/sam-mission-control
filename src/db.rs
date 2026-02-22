//! Database access layer for S.A.M Mission Control.
//!
//! This module provides all MySQL interactions: connection pooling, fleet state
//! queries, chat message storage, task routing, cron job tracking, context
//! snapshots, and spawned-agent records.
//!
//! ## Connection
//! A connection pool is obtained via [`get_pool`]. The pool reads credentials
//! from environment variables (`SAM_DB_URL` or the individual `SAM_DB_*` vars).
//!
//! ## Password Encoding
//! MySQL URLs percent-encode special characters in passwords via
//! [`build_db_url`]. Raw passwords are never written to log output; see
//! [`sanitize_error`].

use mysql_async::prelude::*;

/// Sanitize error messages to remove passwords/credentials
pub fn sanitize_error(msg: &str) -> String {
    // Mask password in mysql:// URLs
    let re_url = regex_lite::Regex::new(r"mysql://[^:]+:([^@]+)@").unwrap();
    let sanitized = re_url.replace_all(msg, "mysql://***:***@").to_string();
    // Mask any password= patterns
    let re_pass = regex_lite::Regex::new(r"(?i)(password|pass|pwd)\s*=\s*\S+").unwrap();
    re_pass.replace_all(&sanitized, "$1=***").to_string()
}
use mysql_async::Pool;
use std::env;

/// Build a MySQL connection URL from individual components, percent-encoding
/// special characters (`$` → `%24`, `@` → `%40`, `#` → `%23`) in the password.
pub fn build_db_url(host: &str, port: &str, user: &str, pass: &str, db: &str) -> String {
    let encoded_pass = pass.replace("$", "%24").replace("@", "%40").replace("#", "%23");
    format!("mysql://{}:{}@{}:{}/{}", user, encoded_pass, host, port, db)
}

/// Create a MySQL connection pool using environment-variable credentials.
///
/// Resolution order:
/// 1. `SAM_DB_URL` — full MySQL URL (highest priority)
/// 2. `SAM_DB_HOST` / `SAM_DB_PORT` / `SAM_DB_USER` / `SAM_DB_PASS` / `SAM_DB_NAME`
/// 3. Defaults: `127.0.0.1:3306`, user `root`, empty password, database `sam_fleet`
pub fn get_pool() -> Pool {
    let url = env::var("SAM_DB_URL")
        .unwrap_or_else(|_| {
            let host = env::var("SAM_DB_HOST").unwrap_or_else(|_| "127.0.0.1".into());
            let port = env::var("SAM_DB_PORT").unwrap_or_else(|_| "3306".into());
            let user = env::var("SAM_DB_USER").unwrap_or_else(|_| "root".into());
            let pass = env::var("SAM_DB_PASS").unwrap_or_else(|_| String::new());
            let db = env::var("SAM_DB_NAME").unwrap_or_else(|_| "sam_fleet".into());
            build_db_url(&host, &port, &user, &pass, &db)
        });
    Pool::new(url.as_str())
}

#[derive(Debug, Clone)]
pub struct DbAgent {
    pub agent_name: String,
    pub hostname: Option<String>,
    pub tailscale_ip: Option<String>,
    pub status: String,
    pub oc_version: Option<String>,
    pub os_info: Option<String>,
    pub kernel: Option<String>,
    pub capabilities: Option<String>,
    pub token_burn_today: i32,
    pub uptime_seconds: i64,
    pub current_task_id: Option<i32>,
    pub gateway_port: i32,
    pub gateway_token: Option<String>,
}

/// Load all agents from `mc_fleet_status`, ordered by name.
pub async fn load_fleet(pool: &Pool) -> Result<Vec<DbAgent>, mysql_async::Error> {
    let mut conn = pool.get_conn().await?;
    let rows: Vec<mysql_async::Row> = conn.query(
        "SELECT agent_name, hostname, tailscale_ip, status, oc_version, os_info, kernel, capabilities, token_burn_today, uptime_seconds, current_task_id, COALESCE(gateway_port,18789), gateway_token FROM mc_fleet_status ORDER BY agent_name",
    ).await?;
    let agents = rows.into_iter().map(|r| {
        DbAgent {
            agent_name: r.get::<Option<String>, _>(0).flatten().unwrap_or_default(),
            hostname: r.get::<Option<String>, _>(1).flatten(),
            tailscale_ip: r.get::<Option<String>, _>(2).flatten(),
            status: r.get::<Option<String>, _>(3).flatten().unwrap_or_else(|| "unknown".into()),
            oc_version: r.get::<Option<String>, _>(4).flatten(),
            os_info: r.get::<Option<String>, _>(5).flatten(),
            kernel: r.get::<Option<String>, _>(6).flatten(),
            capabilities: r.get::<Option<String>, _>(7).flatten(),
            token_burn_today: r.get::<Option<i32>, _>(8).flatten().unwrap_or(0),
            uptime_seconds: r.get::<Option<i64>, _>(9).flatten().unwrap_or(0),
            current_task_id: r.get::<Option<i32>, _>(10).flatten(),
            gateway_port: r.get::<Option<i32>, _>(11).flatten().unwrap_or(18789),
            gateway_token: r.get::<Option<String>, _>(12).flatten(),
        }
    }).collect();
    Ok(agents)
}

/// Update an agent's status fields. Delegates to [`update_agent_status_full`] with `latency_ms = None`.
pub async fn update_agent_status(
    pool: &Pool, agent_name: &str, status: &str,
    os_info: Option<&str>, kernel: Option<&str>, oc_version: Option<&str>,
) -> Result<(), mysql_async::Error> {
    update_agent_status_full(pool, agent_name, status, os_info, kernel, oc_version, None).await
}

/// Update an agent's status, OS info, kernel, OpenClaw version, and optionally latency.
/// Only non-`None` values overwrite existing DB data (`COALESCE` semantics).
pub async fn update_agent_status_full(
    pool: &Pool, agent_name: &str, status: &str,
    os_info: Option<&str>, kernel: Option<&str>, oc_version: Option<&str>,
    latency_ms: Option<u32>,
) -> Result<(), mysql_async::Error> {
    let mut conn = pool.get_conn().await?;
    conn.exec_drop(
        "UPDATE mc_fleet_status SET status=?, os_info=COALESCE(?, os_info), kernel=COALESCE(?, kernel), oc_version=COALESCE(?, oc_version), latency_ms=?, last_heartbeat=NOW(), updated_at=NOW() WHERE agent_name=?",
        (status, os_info, kernel, oc_version, latency_ms, agent_name),
    ).await?;
    Ok(())
}

#[derive(Debug, Clone)]
pub struct ChatMessage {
    pub id: i64,
    pub sender: String,
    pub target: Option<String>,
    pub message: String,
    pub response: Option<String>,
    pub status: String,
    pub kind: String,
    pub created_at: String,
    pub responded_at: Option<String>,
}

/// Send a direct message to a specific agent
pub async fn send_direct(pool: &Pool, sender: &str, target: &str, message: &str) -> Result<i64, mysql_async::Error> {
    let mut conn = pool.get_conn().await?;
    conn.exec_drop(
        "INSERT INTO mc_chat (sender, target, message, status, kind) VALUES (?, ?, ?, 'pending', 'direct')",
        (sender, target, message),
    ).await?;
    let id: Option<i64> = conn.query_first("SELECT LAST_INSERT_ID()").await?;
    Ok(id.unwrap_or(0))
}

/// Send a global broadcast (one row per agent)
pub async fn send_broadcast(pool: &Pool, sender: &str, message: &str, agents: &[String]) -> Result<Vec<i64>, mysql_async::Error> {
    let mut conn = pool.get_conn().await?;
    let mut ids = vec![];
    for agent in agents {
        conn.exec_drop(
            "INSERT INTO mc_chat (sender, target, message, status, kind) VALUES (?, ?, ?, 'pending', 'global')",
            (sender, agent, message),
        ).await?;
        let id: Option<i64> = conn.query_first("SELECT LAST_INSERT_ID()").await?;
        ids.push(id.unwrap_or(0));
    }
    Ok(ids)
}

/// Load global chat (broadcasts only) for dashboard
pub async fn load_global_chat(pool: &Pool, limit: u32) -> Result<Vec<ChatMessage>, mysql_async::Error> {
    let mut conn = pool.get_conn().await?;
    let messages: Vec<ChatMessage> = conn.exec_map(
        "SELECT id, sender, target, message, response, status, kind, DATE_FORMAT(created_at, '%H:%i:%s'), DATE_FORMAT(responded_at, '%H:%i:%s') FROM mc_chat WHERE kind='global' ORDER BY id DESC LIMIT ?",
        (limit,),
        |(id, sender, target, message, response, status, kind, created_at, responded_at)| {
            ChatMessage { id, sender, target, message, response, status, kind, created_at, responded_at }
        },
    ).await?;
    Ok(messages.into_iter().rev().collect())
}

/// Load direct messages for a specific agent
pub async fn load_agent_chat(pool: &Pool, agent: &str, limit: u32) -> Result<Vec<ChatMessage>, mysql_async::Error> {
    let mut conn = pool.get_conn().await?;
    let messages: Vec<ChatMessage> = conn.exec_map(
        "SELECT id, sender, target, message, response, status, kind, DATE_FORMAT(created_at, '%H:%i:%s'), DATE_FORMAT(responded_at, '%H:%i:%s') FROM mc_chat WHERE kind='direct' AND target=? ORDER BY id DESC LIMIT ?",
        (agent, limit),
        |(id, sender, target, message, response, status, kind, created_at, responded_at)| {
            ChatMessage { id, sender, target, message, response, status, kind, created_at, responded_at }
        },
    ).await?;
    Ok(messages.into_iter().rev().collect())
}

/// Legacy: load all chat (for backward compat)
pub async fn load_chat_history(pool: &Pool, limit: u32) -> Result<Vec<ChatMessage>, mysql_async::Error> {
    let mut conn = pool.get_conn().await?;
    let messages: Vec<ChatMessage> = conn.exec_map(
        "SELECT id, sender, target, message, response, status, COALESCE(kind,'global'), DATE_FORMAT(created_at, '%H:%i:%s'), DATE_FORMAT(responded_at, '%H:%i:%s') FROM mc_chat ORDER BY id DESC LIMIT ?",
        (limit,),
        |(id, sender, target, message, response, status, kind, created_at, responded_at)| {
            ChatMessage { id, sender, target, message, response, status, kind, created_at, responded_at }
        },
    ).await?;
    Ok(messages.into_iter().rev().collect())
}

/// Insert a chat message into `mc_chat` and return its row ID.
pub async fn send_chat(pool: &Pool, sender: &str, target: Option<&str>, message: &str) -> Result<i64, mysql_async::Error> {
    let mut conn = pool.get_conn().await?;
    let kind = if target.is_some() { "direct" } else { "global" };
    conn.exec_drop(
        "INSERT INTO mc_chat (sender, target, message, status, kind) VALUES (?, ?, ?, 'pending', ?)",
        (sender, target, message, kind),
    ).await?;
    let id: Option<i64> = conn.query_first("SELECT LAST_INSERT_ID()").await?;
    Ok(id.unwrap_or(0))
}

/// Retrieve all pending chat messages addressed to `agent_name`.
pub async fn get_pending_for_agent(pool: &Pool, agent_name: &str) -> Result<Vec<ChatMessage>, mysql_async::Error> {
    let mut conn = pool.get_conn().await?;
    let messages: Vec<ChatMessage> = conn.exec_map(
        "SELECT id, sender, target, message, response, status, kind, DATE_FORMAT(created_at, '%H:%i:%s'), DATE_FORMAT(responded_at, '%H:%i:%s') FROM mc_chat WHERE target=? AND status='pending' ORDER BY id",
        (agent_name,),
        |(id, sender, target, message, response, status, kind, created_at, responded_at)| {
            ChatMessage { id, sender, target, message, response, status, kind, created_at, responded_at }
        },
    ).await?;
    Ok(messages)
}

/// Mark a chat message as responded, storing the agent's reply and timestamp.
pub async fn respond_to_chat(pool: &Pool, msg_id: i64, response: &str) -> Result<(), mysql_async::Error> {
    let mut conn = pool.get_conn().await?;
    conn.exec_drop(
        "UPDATE mc_chat SET response=?, status='responded', responded_at=NOW(3) WHERE id=?",
        (response, msg_id),
    ).await?;
    Ok(())
}


#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn build_url_basic() {
        let url = build_db_url("10.0.0.1", "3306", "root", "secret", "mydb");
        assert_eq!(url, "mysql://root:secret@10.0.0.1:3306/mydb");
    }

    #[test]
    fn build_url_encodes_dollar() {
        let url = build_db_url("host", "3306", "user", "pa$$word", "db");
        assert_eq!(url, "mysql://user:pa%24%24word@host:3306/db");
    }

    #[test]
    fn build_url_encodes_at() {
        let url = build_db_url("host", "3306", "user", "p@ss", "db");
        assert_eq!(url, "mysql://user:p%40ss@host:3306/db");
    }

    #[test]
    fn build_url_encodes_hash() {
        let url = build_db_url("host", "3306", "user", "p#ss", "db");
        assert_eq!(url, "mysql://user:p%23ss@host:3306/db");
    }

    #[test]
    fn build_url_empty_password() {
        let url = build_db_url("localhost", "3306", "root", "", "test");
        assert_eq!(url, "mysql://root:@localhost:3306/test");
    }

    #[test]
    fn sanitize_masks_url_password() {
        let msg = "Connection failed: mysql://root:MyS3cret@10.0.0.1:3306/db";
        let sanitized = sanitize_error(msg);
        assert!(!sanitized.contains("MyS3cret"));
        assert!(sanitized.contains("***"));
    }

    #[test]
    fn sanitize_masks_password_field() {
        let msg = "Error: password=hunter2 invalid";
        let sanitized = sanitize_error(msg);
        assert!(!sanitized.contains("hunter2"));
        assert!(sanitized.contains("password=***"));
    }

    #[test]
    fn sanitize_preserves_clean_text() {
        let msg = "Connection timeout after 5s";
        assert_eq!(sanitize_error(msg), msg);
    }
}

// ---- Task Board ----

#[derive(Debug, Clone)]
pub struct Task {
    pub id: i32,
    pub description: String,
    pub assigned_agent: Option<String>,
    pub status: String,
    pub priority: i32,
    pub created_by: String,
    pub created_at: String,
    pub result: Option<String>,
}

/// Load the most recent tasks from `mc_task_routing`, ordered by priority then ID.
pub async fn load_tasks(pool: &Pool, limit: u32) -> Result<Vec<Task>, mysql_async::Error> {
    let mut conn = pool.get_conn().await?;
    let tasks: Vec<Task> = conn.exec_map(
        "SELECT id, task_description, assigned_agent, status, priority, COALESCE(created_by,'?'), DATE_FORMAT(created_at, '%m-%d %H:%i'), result FROM mc_task_routing ORDER BY priority ASC, id DESC LIMIT ?",
        (limit,),
        |(id, description, assigned_agent, status, priority, created_by, created_at, result)| {
            Task { id, description, assigned_agent, status, priority, created_by, created_at, result }
        },
    ).await?;
    Ok(tasks)
}

/// Insert a new task into `mc_task_routing`. Status is set to `'assigned'` when
/// `assigned_agent` is provided, otherwise `'queued'`. Returns the new task ID.
pub async fn create_task(pool: &Pool, description: &str, priority: i32, created_by: &str, assigned_agent: Option<&str>) -> Result<i64, mysql_async::Error> {
    let mut conn = pool.get_conn().await?;
    conn.exec_drop(
        "INSERT INTO mc_task_routing (task_description, priority, created_by, assigned_agent, status) VALUES (?, ?, ?, ?, IF(? IS NOT NULL, 'assigned', 'queued'))",
        (description, priority, created_by, assigned_agent, assigned_agent),
    ).await?;
    let id: Option<i64> = conn.query_first("SELECT LAST_INSERT_ID()").await?;
    Ok(id.unwrap_or(0))
}

/// Update the status of a task. Sets `completed_at` for terminal states and
/// `assigned_at` for active transitions.
pub async fn update_task_status(pool: &Pool, task_id: i32, status: &str) -> Result<(), mysql_async::Error> {
    let mut conn = pool.get_conn().await?;
    let extra = match status {
        "completed" | "failed" => ", completed_at=NOW()",
        "assigned" | "running" => ", assigned_at=NOW()",
        _ => "",
    };
    let sql = format!("UPDATE mc_task_routing SET status=?{} WHERE id=?", extra);
    conn.exec_drop(sql, (status, task_id)).await?;
    Ok(())
}

// ── Cron Jobs ──────────────────────────────────────
#[derive(Debug, Clone)]
pub struct AgentCron {
    pub agent_name: String,
    pub cron_id: String,
    pub name: String,
    pub schedule_kind: String,
    pub schedule_value: String,
    pub enabled: bool,
    pub session_target: String,
    pub description: String,
}

/// Load all cron job definitions for `agent` from `mc_agent_crons`.
pub async fn load_agent_crons(pool: &mysql_async::Pool, agent: &str) -> Result<Vec<AgentCron>, mysql_async::Error> {
    let mut conn = pool.get_conn().await?;
    use mysql_async::prelude::*;
    let rows: Vec<mysql_async::Row> = conn.exec(
        "SELECT agent_name, cron_id, name, schedule_kind, schedule_value, enabled, session_target, description FROM mc_agent_crons WHERE agent_name = ? ORDER BY enabled DESC, name",
        (agent,)
    ).await?;
    Ok(rows.into_iter().map(|r| AgentCron {
        agent_name: r.get::<Option<String>, _>(0).flatten().unwrap_or_default(),
        cron_id: r.get::<Option<String>, _>(1).flatten().unwrap_or_default(),
        name: r.get::<Option<String>, _>(2).flatten().unwrap_or_default(),
        schedule_kind: r.get::<Option<String>, _>(3).flatten().unwrap_or_default(),
        schedule_value: r.get::<Option<String>, _>(4).flatten().unwrap_or_default(),
        enabled: r.get::<Option<i32>, _>(5).flatten().unwrap_or(0) != 0,
        session_target: r.get::<Option<String>, _>(6).flatten().unwrap_or_default(),
        description: r.get::<Option<String>, _>(7).flatten().unwrap_or_default(),
    }).collect())
}

/// Insert or update a cron job definition in `mc_agent_crons` (upsert on `agent_name, cron_id`).
pub async fn upsert_agent_cron(pool: &mysql_async::Pool, c: &AgentCron) -> Result<(), mysql_async::Error> {
    let mut conn = pool.get_conn().await?;
    use mysql_async::prelude::*;
    conn.exec_drop(
        "INSERT INTO mc_agent_crons (agent_name, cron_id, name, schedule_kind, schedule_value, enabled, session_target, description, last_collected_at) VALUES (?, ?, ?, ?, ?, ?, ?, ?, NOW()) ON DUPLICATE KEY UPDATE name=VALUES(name), schedule_kind=VALUES(schedule_kind), schedule_value=VALUES(schedule_value), enabled=VALUES(enabled), session_target=VALUES(session_target), description=VALUES(description), last_collected_at=NOW()",
        (&c.agent_name, &c.cron_id, &c.name, &c.schedule_kind, &c.schedule_value, c.enabled as i32, &c.session_target, &c.description)
    ).await?;
    Ok(())
}

// ── Context Snapshots ──────────────────────────────
#[derive(Debug, Clone)]
pub struct AgentContext {
    pub agent_name: String,
    pub session_count: i32,
    pub context_tokens_used: i32,
    pub context_tokens_max: i32,
    pub context_pct: f32,
}

/// Persist a context snapshot for an agent into `mc_agent_context`.
pub async fn save_agent_context(pool: &mysql_async::Pool, ctx: &AgentContext) -> Result<(), mysql_async::Error> {
    let mut conn = pool.get_conn().await?;
    use mysql_async::prelude::*;
    conn.exec_drop(
        "INSERT INTO mc_agent_context (agent_name, session_count, context_tokens_used, context_tokens_max, context_pct) VALUES (?, ?, ?, ?, ?)",
        (&ctx.agent_name, ctx.session_count, ctx.context_tokens_used, ctx.context_tokens_max, ctx.context_pct)
    ).await?;
    Ok(())
}

/// Load the most recent context snapshot for `agent`, or `None` if no record exists.
pub async fn load_latest_context(pool: &mysql_async::Pool, agent: &str) -> Result<Option<AgentContext>, mysql_async::Error> {
    let mut conn = pool.get_conn().await?;
    use mysql_async::prelude::*;
    let rows: Vec<mysql_async::Row> = conn.exec(
        "SELECT agent_name, session_count, context_tokens_used, context_tokens_max, context_pct FROM mc_agent_context WHERE agent_name = ? ORDER BY collected_at DESC LIMIT 1",
        (agent,)
    ).await?;
    Ok(rows.into_iter().next().map(|r| AgentContext {
        agent_name: r.get::<Option<String>, _>(0).flatten().unwrap_or_default(),
        session_count: r.get::<Option<i32>, _>(1).flatten().unwrap_or(0),
        context_tokens_used: r.get::<Option<i32>, _>(2).flatten().unwrap_or(0),
        context_tokens_max: r.get::<Option<i32>, _>(3).flatten().unwrap_or(1000000),
        context_pct: r.get::<Option<f32>, _>(4).flatten().unwrap_or(0.0),
    }))
}

// ── Spawned Agents ─────────────────────────────────
#[derive(Debug, Clone)]
pub struct SpawnedAgent {
    pub id: i64,
    pub agent_name: String,
    pub agent_id: String,
    pub session_key: Option<String>,
    pub prompt: String,
    pub status: String,
    pub response: Option<String>,
    pub created_at: String,
}

/// Queue a sub-agent spawn request in `mc_spawned_agents` with status `'queued'`.
/// Returns the new record ID.
pub async fn spawn_agent(pool: &mysql_async::Pool, agent: &str, agent_id: &str, prompt: &str) -> Result<i64, mysql_async::Error> {
    let mut conn = pool.get_conn().await?;
    use mysql_async::prelude::*;
    conn.exec_drop(
        "INSERT INTO mc_spawned_agents (agent_name, agent_id, prompt, status) VALUES (?, ?, ?, 'queued')",
        (agent, agent_id, prompt)
    ).await?;
    Ok(conn.last_insert_id().unwrap_or(0) as i64)
}

/// Load the 50 most recent spawned-agent records, newest first.
pub async fn load_spawned_agents(pool: &mysql_async::Pool) -> Result<Vec<SpawnedAgent>, mysql_async::Error> {
    let mut conn = pool.get_conn().await?;
    use mysql_async::prelude::*;
    let rows: Vec<mysql_async::Row> = conn.exec(
        "SELECT id, agent_name, agent_id, session_key, prompt, status, response, created_at FROM mc_spawned_agents ORDER BY created_at DESC LIMIT 50",
        ()
    ).await?;
    Ok(rows.into_iter().map(|r| SpawnedAgent {
        id: r.get::<Option<i64>, _>(0).flatten().unwrap_or(0),
        agent_name: r.get::<Option<String>, _>(1).flatten().unwrap_or_default(),
        agent_id: r.get::<Option<String>, _>(2).flatten().unwrap_or("main".into()),
        session_key: r.get::<Option<String>, _>(3).flatten(),
        prompt: r.get::<Option<String>, _>(4).flatten().unwrap_or_default(),
        status: r.get::<Option<String>, _>(5).flatten().unwrap_or("unknown".into()),
        response: r.get::<Option<String>, _>(6).flatten(),
        created_at: r.get::<Option<String>, _>(7).flatten().unwrap_or_default(),
    }).collect())
}
