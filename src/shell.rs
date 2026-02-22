/// Escapes `s` for safe use as a single shell argument.
///
/// The returned string is wrapped in single quotes.  Any single quote
/// characters inside `s` are replaced with the shell sequence `'\''`
/// (close-quote, literal single-quote, open-quote).  No other characters
/// require escaping inside a single-quoted POSIX shell word, so this
/// provides complete protection against shell injection for arbitrary
/// Unicode input.
///
/// # Examples
///
/// ```
/// assert_eq!(shell::escape("hello"), "'hello'");
/// assert_eq!(shell::escape("it's"), "'it'\\''s'");
/// ```
pub fn escape(s: &str) -> String {
    format!("'{}'", s.replace('\'', "'\\''"))
}

#[cfg(test)]
mod tests {
    use super::escape;

    #[test]
    fn plain_string_is_wrapped_in_single_quotes() {
        assert_eq!(escape("hello world"), "'hello world'");
    }

    #[test]
    fn single_quote_is_escaped() {
        assert_eq!(escape("it's"), "'it'\\''s'");
    }

    #[test]
    fn semicolon_injection_is_neutralised() {
        // "; rm -rf /" must not be interpretable as a second command.
        let escaped = escape("; rm -rf /");
        assert_eq!(escaped, "'; rm -rf /'");
        // Confirm the dangerous characters are contained inside quotes.
        assert!(escaped.starts_with('\''));
        assert!(escaped.ends_with('\''));
    }

    #[test]
    fn dollar_subshell_injection_is_neutralised() {
        let escaped = escape("$(whoami)");
        assert_eq!(escaped, "'$(whoami)'");
    }

    #[test]
    fn backtick_injection_is_neutralised() {
        let escaped = escape("`id`");
        assert_eq!(escaped, "'`id`'");
    }

    #[test]
    fn empty_string_produces_empty_single_quoted_pair() {
        assert_eq!(escape(""), "''");
    }

    #[test]
    fn multiple_single_quotes_are_all_escaped() {
        assert_eq!(escape("a'b'c"), "'a'\\''b'\\''c'");
    }

    #[test]
    fn json_payload_with_double_quotes_is_safe() {
        // Typical JSON from serde_json::to_string – double-quotes inside are
        // harmless inside a single-quoted shell word.
        let json = r#"{"role":"user","content":"hello"}"#;
        let escaped = escape(json);
        assert!(escaped.starts_with('\''));
        assert!(escaped.ends_with('\''));
        assert!(escaped.contains(json));
    }
}
