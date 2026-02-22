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

/// Build a MySQL URL from individual components
pub fn build_db_url(host: &str, port: &str, user: &str, pass: &str, db: &str) -> String {
    let encoded_pass = pass.replace("$", "%24").replace("@", "%40").replace("#", "%23");
    format!("mysql://{}:{}@{}:{}/{}", user, encoded_pass, host, port, db)
}

pub fn get_pool() -> Pool {
    let url = env::var("SAM_DB_URL")
        .unwrap_or_else(|_| {
            let host = env::var("SAM_DB_HOST").unwrap_or_else(|_| "127.0.0.1".into());
            let port = env::var("SAM_DB_PORT").unwrap_or_else(|_| "3306".into());
            let user = env::var("SAM_DB_USER").unwrap_or_else(|_| "root".into());
            let pass = env::var("SAM_DB_PASS").unwrap_or_else(|_| String::new());
            let db = env::var("SAM_DB_NAME").unwrap_or_else(|_| "quantum_memory".into());
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

pub async fn load_fleet(pool: &Pool) -> Result<Vec<DbAgent>, mysql_async::Error> {
    let mut conn = pool.get_conn().await?;
    let rows: Vec<mysql_async::Row> = conn.query(
        "SELECT agent_name, hostname, tailscale_ip, status, oc_version, os_info, kernel, capabilities, token_burn_today, uptime_seconds, current_task_id, COALESCE(gateway_port,18789), gateway_token FROM mc_fleet_status ORDER BY agent_name",
    ).await?;
    let agents = rows.into_iter().map(|r| {
        use mysql_async::prelude::FromValue;
        DbAgent {
            agent_name: r.get(0).unwrap_or_default(),
            hostname: r.get(1),
            tailscale_ip: r.get(2),
            status: r.get::<String, _>(3).unwrap_or_else(|| "unknown".into()),
            oc_version: r.get(4),
            os_info: r.get(5),
            kernel: r.get(6),
            capabilities: r.get(7),
            token_burn_today: r.get(8).unwrap_or(0),
            uptime_seconds: r.get(9).unwrap_or(0),
            current_task_id: r.get(10),
            gateway_port: r.get(11).unwrap_or(18789),
            gateway_token: r.get(12),
        }
    }).collect();
    Ok(agents)
}

pub async fn update_agent_status(
    pool: &Pool, agent_name: &str, status: &str,
    os_info: Option<&str>, kernel: Option<&str>, oc_version: Option<&str>,
) -> Result<(), mysql_async::Error> {
    update_agent_status_full(pool, agent_name, status, os_info, kernel, oc_version, None).await
}

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

pub async fn create_task(pool: &Pool, description: &str, priority: i32, created_by: &str, assigned_agent: Option<&str>) -> Result<i64, mysql_async::Error> {
    let mut conn = pool.get_conn().await?;
    conn.exec_drop(
        "INSERT INTO mc_task_routing (task_description, priority, created_by, assigned_agent, status) VALUES (?, ?, ?, ?, IF(? IS NOT NULL, 'assigned', 'queued'))",
        (description, priority, created_by, assigned_agent, assigned_agent),
    ).await?;
    let id: Option<i64> = conn.query_first("SELECT LAST_INSERT_ID()").await?;
    Ok(id.unwrap_or(0))
}

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
