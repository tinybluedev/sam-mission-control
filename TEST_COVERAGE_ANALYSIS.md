# Test Coverage Analysis — S.A.M Mission Control

## Executive Summary

The codebase has **61 tests across 19,849 lines of code** in 8 source files. Test
coverage is heavily concentrated in validation and security-related modules, while
core business logic (TUI, CLI commands, database operations, wizard) remains
largely untested. Additionally, 4 tests in `db.rs` fail to compile because they
reference a removed function (`compute_audit_payload`), preventing `cargo test`
from running at all.

---

## Current Test Inventory

| Module | Lines | Tests | Coverage | Status |
|--------------|-------|-------|----------|--------|
| validate.rs | 446 | 32 | Good | Passing |
| shell.rs | 76 | 8 | Excellent | Passing |
| db.rs | 1,197 | 22 | Partial | **4 broken** |
| config.rs | 184 | 4 | Partial | Passing |
| cli.rs | 1,825 | 4 | Very low | Passing |
| main.rs | 15,079 | 7 | Minimal | Passing |
| wizard.rs | 408 | 0 | None | — |
| theme.rs | 634 | 0 | None | — |

### What Is Well Tested

- **Input validation** (`validate.rs`): 32 tests covering agent names, IP
  addresses, SSH usernames, chat message sanitization, shell arguments, deploy
  filenames, and openclaw config schema. Edge cases and injection attempts are
  thoroughly covered.
- **Shell escaping** (`shell.rs`): 8 tests covering injection vectors (semicolons,
  subshells, backticks, single quotes, empty strings, JSON payloads).
- **DB URL construction and error sanitization** (`db.rs`): Tests for
  percent-encoding special characters in passwords and masking credentials in
  error messages.
- **SQL injection prevention** (`db.rs`): Tests verifying parameterized query
  templates never embed user input.

### What Is Not Tested

The overwhelming majority of the application logic has zero test coverage.

---

## Blocking Issue: Broken Tests

**File:** `src/db.rs:664-676`

Two test functions call `compute_audit_payload()` which does not exist anywhere in
the codebase. This causes 4 compilation errors and prevents `cargo test` from
running entirely:

```
error[E0425]: cannot find function `compute_audit_payload` in this scope
```

**Recommendation:** Either implement the missing function or remove these two
orphaned tests so the rest of the test suite can run.

---

## Proposed Areas for Improvement

### 1. Pure Utility Functions in `main.rs` (High value, low effort)

`main.rs` contains ~30 pure functions with zero side effects that are ideal unit
test candidates. Currently only 4 of them have tests (`format_ram_total`,
`typing_dots`, `splash_prompt`, `splash_scanline_y`). The following are completely
untested:

| Function | Line | What It Does |
|------------------------------|------|---------------------------------------------|
| `format_uptime(secs)` | 391 | Formats seconds as "3d 2h" / "5h 12m" / "7m" |
| `format_app_uptime(secs)` | 407 | Similar, for u64 app uptime |
| `format_last_seen(dt)` | 421 | Extracts HH:MM from a datetime string |
| `os_emoji(os)` | 334 | Maps OS name to emoji ("ubuntu" -> "🟠") |
| `ping_color(latency)` | 466 | Maps latency thresholds to colors |
| `resource_bar(pct, width)` | 595 | Renders a filled/empty progress bar string |
| `mini_bar(pct, width)` | 8223 | Similar bar with percentage label |
| `compute_agent_health_score` | 488 | Computes 0-100 health score from agent metrics |
| `fleet_change_detail(...)` | 357 | Builds change description strings |
| `fuzzy_match(text, query)` | 8246 | Fuzzy search matching |
| `ssh_jump_arg(...)` | 606 | Builds SSH ProxyJump argument |
| `summarize_fleet_search_output`| 647 | Summarizes grep-like output |
| `npm_line_is_meaningful(line)` | 963 | Filters noisy npm output lines |
| `merge_model_list(extra)` | 7500 | Deduplicates and merges model lists |
| `panel_script_from_plugin_entry`| 7462| Extracts panel scripts from JSON config |
| `truncate_str(s, max)` | 7839 | Truncates string with ellipsis |

**Why this matters:** These functions encode business rules (health scoring
thresholds, uptime formatting conventions, fuzzy matching behavior) that are easy
to break silently. They are also trivial to test since they are pure functions with
no dependencies.

**Suggested tests (examples):**

```rust
#[test]
fn format_uptime_shows_days_and_hours() {
    assert_eq!(format_uptime(90061), "1d 1h");
    assert_eq!(format_uptime(3661), "1h 1m");
    assert_eq!(format_uptime(300), "5m");
    assert_eq!(format_uptime(0), "—");
    assert_eq!(format_uptime(-1), "—");
}

#[test]
fn compute_health_score_offline_is_zero() {
    assert_eq!(compute_agent_health_score(
        &AgentStatus::Offline, 0, None, "", "", None, None, None
    ), 0);
}

#[test]
fn compute_health_score_healthy_agent_near_100() {
    let score = compute_agent_health_score(
        &AgentStatus::Online, 86400, Some(50), "1.2.3", "1.2.3",
        Some(30.0), Some(40.0), Some(4096)
    );
    assert!(score >= 90);
}

#[test]
fn fuzzy_match_finds_subsequences() {
    assert!(fuzzy_match("webserver", "wbs").is_some());
    assert!(fuzzy_match("webserver", "xyz").is_none());
    assert!(fuzzy_match("anything", "").is_some()); // empty query matches all
}
```

---

### 2. Agent Wizard Logic (`wizard.rs`) (High value, medium effort)

The `AgentWizard` struct has meaningful state-machine logic (step navigation,
input validation, auto-fill behavior) with **zero tests**. Key testable behaviors:

- **Step navigation:** `advance()` moves forward through steps, `go_back()`
  reverses. At the first step, `go_back()` signals cancel.
- **Input validation per step:** `advance()` validates the current field (calling
  into `validate::normalize_agent_name`, `validate_ip_address`,
  `validate_ssh_username`) and blocks with an error if invalid.
- **Auto-fill:** When advancing past `AgentName`, if `display_name` is empty it
  auto-fills from `agent_name`.
- **Push/pop char:** Location step cycles through options instead of appending.
  Emoji step replaces entire value on each char.
- **Reset:** `open()` resets all state to defaults.

**Suggested tests:**

```rust
#[test]
fn wizard_advance_validates_agent_name() {
    let mut w = AgentWizard::new();
    w.active = true;
    w.agent_name = "".into();
    assert!(!w.advance()); // should fail, stay on step
    assert!(w.error.is_some());
}

#[test]
fn wizard_advance_autofills_display_name() {
    let mut w = AgentWizard::new();
    w.agent_name = "alpha-01".into();
    w.advance();
    assert_eq!(w.display_name, "alpha-01");
}

#[test]
fn wizard_go_back_from_first_step_signals_cancel() {
    let mut w = AgentWizard::new();
    assert!(w.go_back()); // true = cancel
}

#[test]
fn wizard_location_cycles_on_push_char() {
    let mut w = AgentWizard::new();
    w.step = WizardStep::Location;
    assert_eq!(w.location, 0); // Home
    w.push_char('x');
    assert_eq!(w.location, 1); // SM
}
```

---

### 3. Config Loading and Alias Resolution (`config.rs`) (Medium value, medium effort)

Currently tested: `resolve_alias` (2 tests) and `jump_host`/`jump_user` accessors
(2 tests). Not tested:

- **`load_fleet_config()`**: File search across three paths, TOML parsing, error
  propagation. Can be tested by setting `SAM_FLEET_CONFIG` env var to point at a
  temp file.
- **`AgentConfig` accessor defaults**: `display_name()`, `emoji()`, `location()`,
  `ssh_user()` all have default values when the field is `None`.
- **`jump_host()` with empty/whitespace strings**: The implementation filters out
  empty/whitespace jump hosts, but this edge case isn't tested.

**Suggested tests:**

```rust
#[test]
fn agent_config_defaults() {
    let a = AgentConfig {
        name: "test".into(), display: None, emoji: None,
        location: None, ssh_user: None, jump_host: None, jump_user: None,
    };
    assert_eq!(a.display_name(), "test");
    assert_eq!(a.emoji(), "❓");
    assert_eq!(a.location(), "Unknown");
    assert_eq!(a.ssh_user(), "root");
}

#[test]
fn jump_host_whitespace_treated_as_none() {
    let a = AgentConfig {
        name: "x".into(), display: None, emoji: None, location: None,
        ssh_user: None, jump_host: Some("  ".into()), jump_user: None,
    };
    assert_eq!(a.jump_host(), None);
    assert_eq!(a.jump_user(), None);
}

#[test]
fn load_fleet_config_from_env_var() {
    let dir = std::env::temp_dir().join("sam-test-fleet");
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("fleet.toml");
    std::fs::write(&path, "[[agent]]\nname = \"test-01\"\n").unwrap();
    std::env::set_var("SAM_FLEET_CONFIG", path.to_str().unwrap());
    let cfg = load_fleet_config().unwrap();
    assert_eq!(cfg.agent.len(), 1);
    assert_eq!(cfg.agent[0].name, "test-01");
    std::env::remove_var("SAM_FLEET_CONFIG");
    std::fs::remove_dir_all(dir).ok();
}
```

---

### 4. CLI Helper Functions (`cli.rs`) (Medium value, low effort)

Only `parse_semver` and `doctor_exit_code` are tested out of the entire 1,825-line
module. Additional pure/near-pure functions worth testing:

- **`SamConfig` deserialization**: Only `vim_mode` is tested. Other fields like
  `theme`, `bg`, `refresh_secs`, `chat_poll_secs`, `operator_name`, and their
  defaults are untested.
- **`random_hex_token(byte_len)`** (line 890): Generates hex tokens. Should verify
  length and hex-only output.
- **Color helper functions** (lines 24-38): Trivial but could catch ANSI escape
  formatting regressions.

**Suggested tests:**

```rust
#[test]
fn sam_config_defaults_are_sensible() {
    let cfg: SamConfig = toml::from_str("[tui]\n").unwrap();
    assert_eq!(cfg.tui.theme, "standard");
    assert_eq!(cfg.tui.bg, "dark");
    assert_eq!(cfg.tui.refresh_secs, 30);
    assert_eq!(cfg.tui.chat_poll_secs, 3);
    assert_eq!(cfg.operator.name, "operator");
}

#[test]
fn parse_semver_handles_edge_cases() {
    assert_eq!(parse_semver("no version here"), None);
    assert_eq!(parse_semver("v10.20.30-beta"), Some("10.20.30".into()));
    assert_eq!(parse_semver(""), None);
}
```

---

### 5. Theme Module (`theme.rs`) (Low value, low effort)

While themes are mostly data definitions, the state-machine cycling logic is
testable:

```rust
#[test]
fn theme_name_cycles_back_to_start() {
    let mut t = ThemeName::Standard;
    for _ in 0..10 {
        t = t.next();
    }
    assert_eq!(t, ThemeName::Standard); // should wrap around
}

#[test]
fn bg_density_cycles_back_to_start() {
    let mut d = BgDensity::Dark;
    for _ in 0..5 {
        d = d.next();
    }
    assert_eq!(d, BgDensity::Dark);
}

#[test]
fn bg_only_white_is_light() {
    assert!(!BgDensity::Dark.is_light());
    assert!(!BgDensity::Medium.is_light());
    assert!(!BgDensity::Transparent.is_light());
    assert!(BgDensity::White.is_light());
}
```

---

### 6. Database Layer Integration Tests (`db.rs`) (High value, high effort)

The current db.rs tests only verify that SQL strings are static templates and that
`sanitize_error` masks passwords. No actual database operations are tested. The
following would require either a test database or an in-memory mock:

- **`build_db_url` with more special characters**: `%`, `/`, `?`, etc.
- **`load_fleet` / `update_agent_status`**: Round-trip agent data through the DB.
- **`send_chat` / `load_chat_history`**: Verify chat message persistence, ordering.
- **`create_task` / `update_task_status`**: Task lifecycle transitions.
- **`create_operation` / `complete_operation`**: Operations audit trail.
- **`mark_stale_operations_interrupted`**: Stale operation detection.

This is the highest-effort category since it requires database infrastructure in
CI, but it would catch the most consequential bugs (data loss, incorrect queries,
migration failures).

---

## Priority Ranking

| Priority | Area | Effort | Impact |
|----------|------|--------|--------|
| **P0** | Fix broken `compute_audit_payload` tests | 15 min | Unblocks all testing |
| **P1** | Pure utility functions in `main.rs` | 1-2 hrs | Catches logic regressions in health scoring, formatting, fuzzy matching |
| **P2** | Wizard state machine (`wizard.rs`) | 1 hr | Prevents step-navigation and validation bugs during onboarding |
| **P3** | Config loading and defaults (`config.rs`, `cli.rs`) | 1 hr | Catches config parsing regressions |
| **P4** | Theme cycling (`theme.rs`) | 30 min | Low-risk but easy to add |
| **P5** | Database integration tests (`db.rs`) | 4+ hrs | Catches query and migration bugs; requires CI DB setup |

---

## Infrastructure Recommendations

1. **Add `cargo-tarpaulin`** for coverage reporting. Integrate into CI to track
   coverage trends over time.
2. **Create a `tests/` directory** for integration tests that exercise multiple
   modules together (e.g., config loading + validation + wizard flow).
3. **Consider `mockall`** or similar for mocking the database pool in unit tests,
   enabling db.rs function tests without a live MySQL instance.
4. **Set a coverage floor** (e.g., 40% initially) and increase it as new tests are
   added. Enforce it in CI to prevent regressions.
