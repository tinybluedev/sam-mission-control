use mysql_async::prelude::*;
use mysql_async::Pool;
use std::env;

pub fn get_pool() -> Pool {
    let url = env::var("SAM_DB_URL")
        .unwrap_or_else(|_| {
            // Check for individual components
            let host = env::var("SAM_DB_HOST").unwrap_or_else(|_| "127.0.0.1".into());
            let port = env::var("SAM_DB_PORT").unwrap_or_else(|_| "3306".into());
            let user = env::var("SAM_DB_USER").unwrap_or_else(|_| "root".into());
            let pass = env::var("SAM_DB_PASS").unwrap_or_else(|_| String::new());
            let db = env::var("SAM_DB_NAME").unwrap_or_else(|_| "quantum_memory".into());
            let encoded_pass = pass.replace("$", "%24").replace("@", "%40").replace("#", "%23");
            format!("mysql://{}:{}@{}:{}/{}", user, encoded_pass, host, port, db)
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
}

pub async fn load_fleet(pool: &Pool) -> Result<Vec<DbAgent>, mysql_async::Error> {
    let mut conn = pool.get_conn().await?;
    let agents: Vec<DbAgent> = conn.query_map(
        "SELECT agent_name, hostname, tailscale_ip, status, oc_version, os_info, kernel, capabilities, token_burn_today, uptime_seconds, current_task_id FROM mc_fleet_status ORDER BY agent_name",
        |(agent_name, hostname, tailscale_ip, status, oc_version, os_info, kernel, capabilities, token_burn_today, uptime_seconds, current_task_id)| {
            DbAgent { agent_name, hostname, tailscale_ip, status, oc_version, os_info, kernel, capabilities, token_burn_today, uptime_seconds, current_task_id }
        },
    ).await?;
    Ok(agents)
}

pub async fn update_agent_status(
    pool: &Pool, agent_name: &str, status: &str,
    os_info: Option<&str>, kernel: Option<&str>, oc_version: Option<&str>,
) -> Result<(), mysql_async::Error> {
    let mut conn = pool.get_conn().await?;
    conn.exec_drop(
        "UPDATE mc_fleet_status SET status=?, os_info=COALESCE(?, os_info), kernel=COALESCE(?, kernel), oc_version=COALESCE(?, oc_version), last_heartbeat=NOW(), updated_at=NOW() WHERE agent_name=?",
        (status, os_info, kernel, oc_version, agent_name),
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
    pub created_at: String,
    pub responded_at: Option<String>,
}

pub async fn send_chat(pool: &Pool, sender: &str, target: Option<&str>, message: &str) -> Result<i64, mysql_async::Error> {
    let mut conn = pool.get_conn().await?;
    conn.exec_drop(
        "INSERT INTO mc_chat (sender, target, message, status) VALUES (?, ?, ?, 'pending')",
        (sender, target, message),
    ).await?;
    let id: Option<i64> = conn.query_first("SELECT LAST_INSERT_ID()").await?;
    Ok(id.unwrap_or(0))
}

pub async fn load_chat_history(pool: &Pool, limit: u32) -> Result<Vec<ChatMessage>, mysql_async::Error> {
    let mut conn = pool.get_conn().await?;
    let messages: Vec<ChatMessage> = conn.exec_map(
        "SELECT id, sender, target, message, response, status, DATE_FORMAT(created_at, '%H:%i:%s') as created_at, DATE_FORMAT(responded_at, '%H:%i:%s') as responded_at FROM mc_chat ORDER BY id DESC LIMIT ?",
        (limit,),
        |(id, sender, target, message, response, status, created_at, responded_at)| {
            ChatMessage { id, sender, target, message, response, status, created_at, responded_at }
        },
    ).await?;
    Ok(messages.into_iter().rev().collect())
}

pub async fn get_pending_for_agent(pool: &Pool, agent_name: &str) -> Result<Vec<ChatMessage>, mysql_async::Error> {
    let mut conn = pool.get_conn().await?;
    let messages: Vec<ChatMessage> = conn.exec_map(
        "SELECT id, sender, target, message, response, status, DATE_FORMAT(created_at, '%H:%i:%s'), DATE_FORMAT(responded_at, '%H:%i:%s') FROM mc_chat WHERE target=? AND status='pending' ORDER BY id",
        (agent_name,),
        |(id, sender, target, message, response, status, created_at, responded_at)| {
            ChatMessage { id, sender, target, message, response, status, created_at, responded_at }
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
