/// Input validation and sanitization for SAM Mission Control.
///
/// All user-supplied values must pass through these functions before being
/// used in shell commands, SQL statements, or stored in the database.

/// Normalize and validate an agent name in one step.
///
/// Trims whitespace, lowercases, and replaces spaces with hyphens before
/// applying `validate_agent_name`.  Returns the normalized name on success.
pub fn normalize_agent_name(name: &str) -> Result<String, String> {
    let normalized = name.trim().to_lowercase().replace(' ', "-");
    validate_agent_name(&normalized)?;
    Ok(normalized)
}

/// Maximum allowed length for an agent name.
pub const AGENT_NAME_MAX_LEN: usize = 64;

/// Validate an agent name: lowercase alphanumeric characters and hyphens only,
/// 1–64 characters, must not start or end with a hyphen.
///
/// Returns `Ok(())` on success or `Err` with a human-readable reason.
pub fn validate_agent_name(name: &str) -> Result<(), String> {
    if name.is_empty() {
        return Err("Agent name must not be empty".into());
    }
    if name.len() > AGENT_NAME_MAX_LEN {
        return Err(format!("Agent name must be ≤ {} characters", AGENT_NAME_MAX_LEN));
    }
    if name.starts_with('-') || name.ends_with('-') {
        return Err("Agent name must not start or end with a hyphen".into());
    }
    if !name.chars().all(|c| c.is_ascii_alphanumeric() || c == '-') {
        return Err("Agent name may only contain letters, digits, and hyphens".into());
    }
    Ok(())
}

/// Validate an IPv4 address string (four octets, each 0–255).
/// Also accepts plain hostnames (alphanumeric + hyphens + dots) for flexibility.
///
/// Returns `Ok(())` on success or `Err` with a human-readable reason.
pub fn validate_ip_address(addr: &str) -> Result<(), String> {
    if addr.is_empty() {
        return Err("Host/IP must not be empty".into());
    }
    // Try strict IPv4 first
    let parts: Vec<&str> = addr.split('.').collect();
    if parts.len() == 4 {
        let all_octets = parts.iter().all(|p| {
            !p.is_empty() && p.parse::<u8>().is_ok()
        });
        if all_octets {
            return Ok(());
        }
        return Err("Invalid IPv4 address — each octet must be 0–255".into());
    }
    // Accept hostnames: alphanumeric, hyphens, dots only
    if addr.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '.') {
        return Ok(());
    }
    Err("Host must be a valid IPv4 address or hostname (letters, digits, hyphens, dots)".into())
}

/// Characters forbidden in SSH usernames to prevent command injection.
const SSH_USER_FORBIDDEN: &[char] = &[
    ';', '|', '&', '\\', '$', '`', '>', '<', '(', ')',
    '\'', '"', ' ', '\t', '\n', '\r', '!', '*', '?', '{', '}', '[', ']', '#',
];

/// Validate an SSH username: no shell metacharacters, 1–32 characters,
/// must start with a letter or underscore.
///
/// Returns `Ok(())` on success or `Err` with a human-readable reason.
pub fn validate_ssh_username(user: &str) -> Result<(), String> {
    if user.is_empty() {
        return Err("SSH username must not be empty".into());
    }
    if user.len() > 32 {
        return Err("SSH username must be ≤ 32 characters".into());
    }
    match user.chars().next() {
        Some(c) if c.is_ascii_alphabetic() || c == '_' => {}
        _ => return Err("SSH username must start with a letter or underscore".into()),
    }
    if let Some(bad) = user.chars().find(|c| SSH_USER_FORBIDDEN.contains(c)) {
        return Err(format!("SSH username contains forbidden character: {:?}", bad));
    }
    Ok(())
}

/// Sanitize a chat message before storing it in the database.
///
/// Escapes HTML-significant characters to prevent stored XSS if the data
/// is ever rendered in a web UI.  The stored value is the escaped form.
pub fn sanitize_chat_message(msg: &str) -> String {
    // Trim leading/trailing whitespace first
    let msg = msg.trim();
    let mut out = String::with_capacity(msg.len());
    for c in msg.chars() {
        match c {
            '&'  => out.push_str("&amp;"),
            '<'  => out.push_str("&lt;"),
            '>'  => out.push_str("&gt;"),
            '"'  => out.push_str("&quot;"),
            '\'' => out.push_str("&#x27;"),
            _    => out.push(c),
        }
    }
    out
}

/// Characters forbidden in values that will be interpolated into shell commands.
const SHELL_FORBIDDEN: &[char] = &[
    ';', '|', '&', '\\', '$', '`', '>', '<', '(', ')', '\n', '\r',
    '\'', '"', ' ', '\t', '!', '*', '?', '{', '}', '[', ']',
];

/// Validate a string that will be passed as a shell argument.
///
/// Returns `Ok(())` if the value contains no shell metacharacters,
/// or `Err` describing the offending character.
pub fn validate_shell_arg(value: &str) -> Result<(), String> {
    if let Some(bad) = value.chars().find(|c| SHELL_FORBIDDEN.contains(c)) {
        return Err(format!("Value contains forbidden shell character: {:?}", bad));
    }
    Ok(())
}

/// Validate a deploy file name used for remote workspace writes.
///
/// Accepts only simple filenames (no path separators or traversal segments).
pub fn validate_deploy_filename(value: &str) -> Result<(), String> {
    if value.is_empty() {
        return Err("File name must not be empty".into());
    }
    if value.contains('/') || value.contains('\\') || value == "." || value == ".." || value.contains("..") {
        return Err("File must be a simple name (no path separators or traversal)".into());
    }
    validate_shell_arg(value)
}

// ---- Unit tests ----

#[cfg(test)]
mod tests {
    use super::*;

    // ── validate_agent_name ──────────────────────────────────────────────────

    #[test]
    fn agent_name_valid() {
        assert!(validate_agent_name("alpha-01").is_ok());
        assert!(validate_agent_name("agent").is_ok());
        assert!(validate_agent_name("a").is_ok());
        assert!(validate_agent_name(&"x".repeat(64)).is_ok());
    }

    #[test]
    fn agent_name_empty() {
        assert!(validate_agent_name("").is_err());
    }

    #[test]
    fn agent_name_too_long() {
        assert!(validate_agent_name(&"a".repeat(65)).is_err());
    }

    #[test]
    fn agent_name_leading_hyphen() {
        assert!(validate_agent_name("-agent").is_err());
    }

    #[test]
    fn agent_name_trailing_hyphen() {
        assert!(validate_agent_name("agent-").is_err());
    }

    #[test]
    fn agent_name_special_chars() {
        assert!(validate_agent_name("agent name").is_err()); // space
        assert!(validate_agent_name("agent;name").is_err()); // semicolon
        assert!(validate_agent_name("agent|name").is_err()); // pipe
        assert!(validate_agent_name("agent$name").is_err()); // dollar
    }

    // ── validate_ip_address ─────────────────────────────────────────────────

    #[test]
    fn ip_valid_ipv4() {
        assert!(validate_ip_address("192.168.1.1").is_ok());
        assert!(validate_ip_address("10.0.0.1").is_ok());
        assert!(validate_ip_address("0.0.0.0").is_ok());
        assert!(validate_ip_address("255.255.255.255").is_ok());
    }

    #[test]
    fn ip_valid_hostname() {
        assert!(validate_ip_address("my-host").is_ok());
        assert!(validate_ip_address("host.example.com").is_ok());
    }

    #[test]
    fn ip_empty() {
        assert!(validate_ip_address("").is_err());
    }

    #[test]
    fn ip_bad_octet() {
        assert!(validate_ip_address("256.0.0.1").is_err());
        assert!(validate_ip_address("192.168.1.256").is_err());
    }

    #[test]
    fn ip_special_chars() {
        assert!(validate_ip_address("10.0.0.1;id").is_err());
        assert!(validate_ip_address("host$(cmd)").is_err());
    }

    // ── validate_ssh_username ───────────────────────────────────────────────

    #[test]
    fn ssh_user_valid() {
        assert!(validate_ssh_username("admin").is_ok());
        assert!(validate_ssh_username("root").is_ok());
        assert!(validate_ssh_username("deploy_user").is_ok());
        assert!(validate_ssh_username("user123").is_ok());
    }

    #[test]
    fn ssh_user_empty() {
        assert!(validate_ssh_username("").is_err());
    }

    #[test]
    fn ssh_user_too_long() {
        assert!(validate_ssh_username(&"a".repeat(33)).is_err());
    }

    #[test]
    fn ssh_user_injection_chars() {
        assert!(validate_ssh_username("user;id").is_err());
        assert!(validate_ssh_username("user|cat /etc/passwd").is_err());
        assert!(validate_ssh_username("user&cmd").is_err());
        assert!(validate_ssh_username("$(id)").is_err());
        assert!(validate_ssh_username("`id`").is_err());
        assert!(validate_ssh_username("user name").is_err()); // space
    }

    #[test]
    fn ssh_user_starts_with_digit() {
        assert!(validate_ssh_username("1user").is_err());
    }

    #[test]
    fn ssh_user_starts_with_underscore() {
        assert!(validate_ssh_username("_svc").is_ok());
    }

    // ── normalize_agent_name ────────────────────────────────────────────────

    #[test]
    fn normalize_lowercases_and_replaces_spaces() {
        assert_eq!(normalize_agent_name("My Agent").unwrap(), "my-agent");
    }

    #[test]
    fn normalize_trims_whitespace() {
        assert_eq!(normalize_agent_name("  alpha  ").unwrap(), "alpha");
    }

    #[test]
    fn normalize_rejects_empty_after_trim() {
        assert!(normalize_agent_name("   ").is_err());
    }

    // ── IPv4 edge cases ─────────────────────────────────────────────────────

    #[test]
    fn ip_empty_octet() {
        assert!(validate_ip_address("1.2.3.").is_err());
        assert!(validate_ip_address("1..2.3").is_err());
    }

    #[test]
    fn chat_sanitize_normal_text() {
        assert_eq!(sanitize_chat_message("hello world"), "hello world");
    }

    #[test]
    fn chat_sanitize_html_chars() {
        let out = sanitize_chat_message("<script>alert('xss')</script>");
        assert!(!out.contains('<'));
        assert!(!out.contains('>'));
        assert!(!out.contains('\''));
        assert!(out.contains("&lt;"));
        assert!(out.contains("&gt;"));
    }

    #[test]
    fn chat_sanitize_ampersand() {
        assert_eq!(sanitize_chat_message("a & b"), "a &amp; b");
    }

    #[test]
    fn chat_sanitize_trims_whitespace() {
        assert_eq!(sanitize_chat_message("  hello  "), "hello");
    }

    #[test]
    fn chat_sanitize_empty() {
        assert_eq!(sanitize_chat_message(""), "");
    }

    // ── validate_shell_arg ──────────────────────────────────────────────────

    #[test]
    fn shell_arg_valid() {
        assert!(validate_shell_arg("hello").is_ok());
        assert!(validate_shell_arg("192.168.1.1").is_ok());
        assert!(validate_shell_arg("my-host_01").is_ok());
    }

    #[test]
    fn shell_arg_forbidden_chars() {
        assert!(validate_shell_arg("val;cmd").is_err());
        assert!(validate_shell_arg("val|cmd").is_err());
        assert!(validate_shell_arg("val&cmd").is_err());
        assert!(validate_shell_arg("val`cmd`").is_err());
        assert!(validate_shell_arg("$(cmd)").is_err());
        assert!(validate_shell_arg("val>out").is_err());
        assert!(validate_shell_arg("val<in").is_err());
    }

    // ── validate_deploy_filename ────────────────────────────────────────────

    #[test]
    fn deploy_filename_valid() {
        assert!(validate_deploy_filename("SOUL.md").is_ok());
        assert!(validate_deploy_filename("agent-config.json").is_ok());
    }

    #[test]
    fn deploy_filename_rejects_path_and_traversal() {
        assert!(validate_deploy_filename("../secret").is_err());
        assert!(validate_deploy_filename("a/b.txt").is_err());
        assert!(validate_deploy_filename("a\\b.txt").is_err());
        assert!(validate_deploy_filename("..").is_err());
    }
}
