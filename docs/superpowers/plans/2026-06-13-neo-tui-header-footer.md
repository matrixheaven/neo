# Neo TUI Header/Footer Redesign Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Remove Neo’s persistent top header, replace it with a startup banner in the transcript, and redesign the footer as a two-line, semantic-color status bar.

**Architecture:** Add `workspace_root` and semantic footer colors to `NeoTuiApp`, introduce a `TranscriptItem::Banner` variant, collapse `AppLayout` to body + prompt + two-line footer, and rewrite `render_footer` to build left/right `Line` widgets colored by theme. The interactive mode passes workspace root and pushes the banner on startup.

**Tech Stack:** Rust 2024, ratatui, serde, cargo test, cargo clippy

---

## File Structure

| File | Responsibility |
|---|---|
| `crates/neo-tui/src/app.rs` | `NeoTuiApp` state, `TuiTheme` semantic colors, `TranscriptItem::Banner` variant, helper methods for footer labels. |
| `crates/neo-tui/src/components.rs` | Layout geometry (`app_layout`), footer rendering (`render_footer`), main `Widget` impl, banner render path. |
| `crates/neo-tui/src/transcript_renderer.rs` | (if exists) Render `TranscriptItem::Banner` as a boxed card; otherwise add inline render logic in `components.rs`. |
| `crates/neo-agent/src/themes.rs` | Deserialize new footer semantic color keys from theme JSON. |
| `crates/neo-agent/src/modes/interactive.rs` | Pass `workspace_root` into `NeoTuiApp::new`; push `TranscriptItem::Banner` on startup. |
| `crates/neo-tui/tests/app_shell.rs` | Update assertions: no top header, two-line footer, banner present, context thresholds. |

---

### Task 1: Add `workspace_root` to `NeoTuiApp`

**Files:**
- Modify: `crates/neo-tui/src/app.rs:243-269` (`NeoTuiApp` struct)
- Modify: `crates/neo-tui/src/app.rs:273-310` (`NeoTuiApp::new`)
- Modify: `crates/neo-tui/src/app.rs` (add getter `workspace_root()`)
- Test: `crates/neo-tui/tests/app_shell.rs` (update constructors)

- [ ] **Step 1: Add field and constructor parameter**

```rust
// In crates/neo-tui/src/app.rs, inside pub struct NeoTuiApp
pub struct NeoTuiApp {
    title: String,
    session_label: String,
    model_label: String,
    workspace_root: PathBuf,
    // ... rest unchanged
}
```

```rust
// In NeoTuiApp::new
pub fn new(
    title: impl Into<String>,
    session_label: impl Into<String>,
    model_label: impl Into<String>,
    workspace_root: impl Into<PathBuf>,
) -> Self {
    Self {
        title: title.into(),
        session_label: session_label.into(),
        model_label: model_label.into(),
        workspace_root: workspace_root.into(),
        // ... rest unchanged
    }
}
```

Add `use std::path::PathBuf;` at the top of `crates/neo-tui/src/app.rs` if not already present.

- [ ] **Step 2: Add getter**

```rust
impl NeoTuiApp {
    // near other getters
    #[must_use]
    pub fn workspace_root(&self) -> &PathBuf {
        &self.workspace_root
    }
}
```

- [ ] **Step 3: Update test constructors**

```rust
// In crates/neo-tui/tests/app_shell.rs
// Replace every:
let mut app = NeoTuiApp::new("neo", "session-a", "openai/gpt-4.1");
// with:
let mut app = NeoTuiApp::new("neo", "session-a", "openai/gpt-4.1", "/tmp/neo-ws");
```

Use `rg 'NeoTuiApp::new\(' crates/neo-tui/tests` to find all occurrences.

- [ ] **Step 4: Run crate tests to verify signature change compiles**

```bash
cargo test -p neo-tui --test app_shell 2>&1 | tail -40
```

Expected: compile succeeds; existing tests still pass (header/footer content assertions will fail later, but signature errors should be gone).

- [ ] **Step 5: Commit**

```bash
git add crates/neo-tui/src/app.rs crates/neo-tui/tests/app_shell.rs
git commit -m "refactor(tui): add workspace_root to NeoTuiApp"
```

---

### Task 2: Add `TranscriptItem::Banner` variant

**Files:**
- Modify: `crates/neo-tui/src/app.rs:2081-2113` (`TranscriptItem` enum)
- Modify: `crates/neo-tui/src/app.rs` (add constructor)
- Modify: `crates/neo-tui/src/components.rs` (render banner)

- [ ] **Step 1: Extend the enum**

```rust
pub enum TranscriptItem {
    // ... existing variants
    Notice {
        content: String,
    },
    Banner {
        title: String,
        session_label: String,
        model_label: String,
        workspace_root: PathBuf,
    },
}
```

- [ ] **Step 2: Add a constructor method**

```rust
impl TranscriptItem {
    #[must_use]
    pub fn banner(
        title: impl Into<String>,
        session_label: impl Into<String>,
        model_label: impl Into<String>,
        workspace_root: impl Into<PathBuf>,
    ) -> Self {
        Self::Banner {
            title: title.into(),
            session_label: session_label.into(),
            model_label: model_label.into(),
            workspace_root: workspace_root.into(),
        }
    }
}
```

- [ ] **Step 3: Render the banner in `TranscriptWidget`**

Locate the `TranscriptRenderer` (or the inline transcript rendering logic). Find where `TranscriptItem::Notice` is handled and add a `Banner` arm that draws a boxed card:

```rust
TranscriptItem::Banner { title, session_label, model_label, workspace_root } => {
    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(theme.surface_border))
        .style(Style::default().bg(theme.background));
    let inner = block.inner(area);
    block.render(area, buf);
    let mut y = inner.y;
    write_line(inner, buf, y, title, Style::default().fg(theme.header).add_modifier(Modifier::BOLD));
    y += 1;
    write_line(inner, buf, y, &format!("Session: {session_label}   Model: {model_label}"), Style::default().fg(theme.muted));
    y += 1;
    write_line(inner, buf, y, &workspace_root.display().to_string(), Style::default().fg(theme.muted));
}
```

Adjust `write_line` arguments to match the existing signature in `components.rs`.

- [ ] **Step 4: Verify banner renders**

```bash
cargo test -p neo-tui --lib banner 2>&1 | tail -30
```

If there is no existing banner test, skip to Task 9 where tests are added.

- [ ] **Step 5: Commit**

```bash
git add crates/neo-tui/src/app.rs crates/neo-tui/src/components.rs
git commit -m "feat(tui): add TranscriptItem::Banner variant and render path"
```

---

### Task 3: Add semantic footer colors to `TuiTheme`

**Files:**
- Modify: `crates/neo-tui/src/app.rs:15-49` (`TuiTheme` struct)
- Modify: `crates/neo-tui/src/app.rs:51-88` (`Default` impl)
- Modify: `crates/neo-tui/src/app.rs:90-191` (add `with_*` methods)

- [ ] **Step 1: Add fields**

```rust
pub struct TuiTheme {
    // existing fields ...
    pub overlay_border: Color,

    // Footer semantic colors
    pub footer_permission_allow: Color,
    pub footer_permission_ask: Color,
    pub footer_permission_deny: Color,
    pub footer_working: Color,
    pub footer_context_ok: Color,
    pub footer_context_warn: Color,
    pub footer_context_critical: Color,
    pub footer_hint: Color,
}
```

- [ ] **Step 2: Add defaults**

```rust
impl Default for TuiTheme {
    fn default() -> Self {
        Self {
            // existing fields ...
            overlay_border: Color::Rgb(88, 166, 255),

            footer_permission_allow: Color::Rgb(65, 184, 131),   // success
            footer_permission_ask: Color::Rgb(88, 166, 255),     // accent
            footer_permission_deny: Color::Rgb(248, 81, 73),     // danger
            footer_working: Color::Rgb(88, 166, 255),            // accent
            footer_context_ok: Color::Rgb(139, 148, 158),        // muted
            footer_context_warn: Color::Rgb(210, 153, 34),       // warning
            footer_context_critical: Color::Rgb(248, 81, 73),    // danger
            footer_hint: Color::Rgb(139, 148, 158),              // muted
        }
    }
}
```

- [ ] **Step 3: Add builder methods**

```rust
impl TuiTheme {
    // ... existing with_* methods ...

    #[must_use]
    pub const fn with_footer_permission_allow(mut self, color: Color) -> Self {
        self.footer_permission_allow = color;
        self
    }

    #[must_use]
    pub const fn with_footer_permission_ask(mut self, color: Color) -> Self {
        self.footer_permission_ask = color;
        self
    }

    #[must_use]
    pub const fn with_footer_permission_deny(mut self, color: Color) -> Self {
        self.footer_permission_deny = color;
        self
    }

    #[must_use]
    pub const fn with_footer_working(mut self, color: Color) -> Self {
        self.footer_working = color;
        self
    }

    #[must_use]
    pub const fn with_footer_context_ok(mut self, color: Color) -> Self {
        self.footer_context_ok = color;
        self
    }

    #[must_use]
    pub const fn with_footer_context_warn(mut self, color: Color) -> Self {
        self.footer_context_warn = color;
        self
    }

    #[must_use]
    pub const fn with_footer_context_critical(mut self, color: Color) -> Self {
        self.footer_context_critical = color;
        self
    }

    #[must_use]
    pub const fn with_footer_hint(mut self, color: Color) -> Self {
        self.footer_hint = color;
        self
    }
}
```

- [ ] **Step 4: Compile**

```bash
cargo check -p neo-tui 2>&1 | tail -30
```

Expected: no errors.

- [ ] **Step 5: Commit**

```bash
git add crates/neo-tui/src/app.rs
git commit -m "feat(tui): add semantic footer colors to TuiTheme"
```

---

### Task 4: Deserialize new theme colors from JSON

**Files:**
- Modify: `crates/neo-agent/src/themes.rs:35-56` (`ThemeColors` struct)
- Modify: `crates/neo-agent/src/themes.rs:246-336` (`apply_colors`)

- [ ] **Step 1: Add optional fields**

```rust
#[derive(Debug, Default, Deserialize)]
struct ThemeColors {
    // existing fields ...
    overlay_border: Option<String>,

    footer_permission_allow: Option<String>,
    footer_permission_ask: Option<String>,
    footer_permission_deny: Option<String>,
    footer_working: Option<String>,
    footer_context_ok: Option<String>,
    footer_context_warn: Option<String>,
    footer_context_critical: Option<String>,
    footer_hint: Option<String>,
}
```

- [ ] **Step 2: Apply each color in `apply_colors`**

After the existing `apply_color(&mut theme.overlay_border, ...)` call, add:

```rust
apply_color(
    &mut theme.footer_permission_allow,
    "footer_permission_allow",
    colors.footer_permission_allow.as_deref(),
    path,
)?;
apply_color(
    &mut theme.footer_permission_ask,
    "footer_permission_ask",
    colors.footer_permission_ask.as_deref(),
    path,
)?;
apply_color(
    &mut theme.footer_permission_deny,
    "footer_permission_deny",
    colors.footer_permission_deny.as_deref(),
    path,
)?;
apply_color(
    &mut theme.footer_working,
    "footer_working",
    colors.footer_working.as_deref(),
    path,
)?;
apply_color(
    &mut theme.footer_context_ok,
    "footer_context_ok",
    colors.footer_context_ok.as_deref(),
    path,
)?;
apply_color(
    &mut theme.footer_context_warn,
    "footer_context_warn",
    colors.footer_context_warn.as_deref(),
    path,
)?;
apply_color(
    &mut theme.footer_context_critical,
    "footer_context_critical",
    colors.footer_context_critical.as_deref(),
    path,
)?;
apply_color(
    &mut theme.footer_hint,
    "footer_hint",
    colors.footer_hint.as_deref(),
    path,
)?;
```

- [ ] **Step 3: Compile**

```bash
cargo check -p neo-agent 2>&1 | tail -30
```

Expected: no errors.

- [ ] **Step 4: Commit**

```bash
git add crates/neo-agent/src/themes.rs
git commit -m "feat(themes): deserialize footer semantic colors from JSON"
```

---

### Task 5: Remove header row and make footer two rows in layout

**Files:**
- Modify: `crates/neo-tui/src/components.rs:31-98` (`app_layout`)

- [ ] **Step 1: Update `app_layout` geometry**

Current code reserves 1 row at the top for the header. Change it to reserve 0 header rows and 2 footer rows (or 1/0 on short terminals).

```rust
pub fn app_layout(app: &NeoTuiApp, area: Rect) -> AppLayout {
    let prompt_height = prompt_height(app.prompt(), area.width);
    let footer_bar_height = if area.height >= 12 {
        2
    } else if area.height >= 8 {
        1
    } else {
        0
    };
    // ... rest unchanged until body_y
    let body_y = area.y; // no header offset
    let body_height = area.height.saturating_sub(bottom_height);
    // ...
}
```

Remove the `let body_y = area.y.saturating_add(1);` line and use `area.y` directly.

- [ ] **Step 2: Verify layout tests**

```bash
cargo test -p neo-tui --test app_shell 2>&1 | tail -30
```

Expected: tests compile; some may fail because footer content is still one line.

- [ ] **Step 3: Commit**

```bash
git add crates/neo-tui/src/components.rs
git commit -m "feat(tui): remove header row and reserve two footer rows"
```

---

### Task 6: Rewrite `render_footer` as two semantic lines

**Files:**
- Modify: `crates/neo-tui/src/components.rs:1185-1214` (`render_footer`)
- Modify: `crates/neo-tui/src/app.rs` (add footer helper methods)

- [ ] **Step 1: Add footer helper methods to `NeoTuiApp`**

```rust
impl NeoTuiApp {
    #[must_use]
    pub fn permission_badge(&self) -> (&'static str, Color) {
        match self.permission_decision {
            PermissionDecision::Allow => ("allow", self.theme.footer_permission_allow),
            PermissionDecision::Ask => ("ask", self.theme.footer_permission_ask),
            PermissionDecision::Deny => ("deny", self.theme.footer_permission_deny),
        }
    }

    #[must_use]
    pub fn cwd_label(&self) -> String {
        let path = self.workspace_root.display().to_string();
        if let Some(home) = std::env::var_os("HOME") {
            let home = PathBuf::from(home);
            if let Ok(stripped) = self.workspace_root.strip_prefix(&home) {
                return format!("~/{}", stripped.display());
            }
        }
        path
    }

    #[must_use]
    pub fn context_color(&self) -> Color {
        let Some(window) = self.context_window else {
            return self.theme.footer_context_ok;
        };
        let max = window.max_tokens().max(1);
        let used = window.used_tokens();
        let pct = used * 100 / max;
        if pct >= 90 {
            self.theme.footer_context_critical
        } else if pct >= 70 {
            self.theme.footer_context_warn
        } else {
            self.theme.footer_context_ok
        }
    }
}
```

Add `use std::path::PathBuf;` if not already present.

- [ ] **Step 2: Rewrite `render_footer`**

```rust
fn render_footer(app: &NeoTuiApp, area: Rect, buf: &mut Buffer) {
    if area.width == 0 || area.height == 0 {
        return;
    }

    buf.set_style(area, Style::default().bg(app.theme().background));

    let width = usize::from(area.width);

    // Line 1: status
    if area.height >= 1 {
        let (badge, badge_color) = app.permission_badge();
        let badge_span = Span::styled(format!("[{badge}]"), Style::default().fg(badge_color));

        let working_span = app.working_label().map(|label| {
            Span::styled(
                format!("● {label}"),
                Style::default().fg(app.theme().footer_working),
            )
        });

        let cwd_span = Span::styled(app.cwd_label(), Style::default().fg(app.theme().muted));

        let mut status_spans: Vec<Span> = vec![badge_span];
        if let Some(working) = working_span {
            status_spans.push(Span::raw("  "));
            status_spans.push(working);
        }
        status_spans.push(Span::raw("  "));
        status_spans.push(cwd_span);

        let status_line = Line::from(status_spans);
        let status_area = Rect { x: area.x, y: area.y, width: area.width, height: 1 };
        render_truncated_line(status_area, buf, &status_line, Alignment::Left);
    }

    // Line 2: hints + context
    if area.height >= 2 {
        let mut hint_spans = vec![
            Span::styled(
                "enter send · shift+enter newline · / commands",
                Style::default().fg(app.theme().footer_hint),
            ),
        ];

        let context_spans = app.context_window_label().map(|label| {
            let color = app.context_color();
            vec![
                Span::styled(label, Style::default().fg(color)),
            ]
        });

        if !app.transcript_view().is_following_tail() {
            hint_spans.push(Span::raw("  ·  "));
            hint_spans.push(Span::styled(
                "new output below · end to jump",
                Style::default().fg(app.theme().footer_hint),
            ));
        }

        let hint_line = Line::from(hint_spans);
        let hint_area = Rect { x: area.x, y: area.y + 1, width: area.width, height: 1 };

        // Render left hints
        render_truncated_line(hint_area, buf, &hint_line, Alignment::Left);

        // Render right context, overwriting rightmost cells
        if let Some(ctx) = context_spans {
            let ctx_line = Line::from(ctx);
            render_truncated_line_right(hint_area, buf, &ctx_line);
        }
    }
}
```

- [ ] **Step 3: Add helper render functions and imports**

Add these imports at the top of `crates/neo-tui/src/components.rs`:

```rust
use ratatui::{
    buffer::Buffer,
    layout::{Alignment, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Clear, Paragraph, Widget},
};
```

Add the helper functions:

```rust
fn render_truncated_line(area: Rect, buf: &mut Buffer, line: &Line<'_>, alignment: Alignment) {
    Paragraph::new(line.clone())
        .alignment(alignment)
        .render(area, buf);
}

fn render_truncated_line_right(area: Rect, buf: &mut Buffer, line: &Line<'_>) {
    let text: String = line.spans.iter().map(|span| span.content.to_string()).collect();
    let width = visible_width(&text);
    if width == 0 {
        return;
    }
    let x = area.right().saturating_sub(width.min(area.width as usize) as u16);
    let area = Rect {
        x,
        y: area.y,
        width: area.width - (x - area.x),
        height: 1,
    };
    render_truncated_line(area, buf, line, Alignment::Right);
}
```

Use the existing `visible_width` helper from `components.rs`.

- [ ] **Step 4: Compile and run tests**

```bash
cargo test -p neo-tui --test app_shell 2>&1 | tail -50
```

Expected: tests compile. Some tests that asserted old footer strings will fail until Task 9.

- [ ] **Step 5: Commit**

```bash
git add crates/neo-tui/src/components.rs crates/neo-tui/src/app.rs
git commit -m "feat(tui): rewrite footer as two semantic lines"
```

---

### Task 7: Remove header render block from `Widget` impl

**Files:**
- Modify: `crates/neo-tui/src/components.rs:1033-1104` (`impl Widget for &NeoTuiApp`)

- [ ] **Step 1: Delete header drawing code**

Remove this block from `impl Widget for &NeoTuiApp`:

```rust
// DELETE from components.rs
let mut header_parts = vec![
    self.title().to_owned(),
    format!("session:{}", self.session_label()),
    format!("model:{}", self.model_label()),
];
if let Some(context) = self.context_window_label() {
    header_parts.push(context);
}
if let Some(working) = self.working_label() {
    header_parts.push(format!("● {working}"));
}
let header = header_parts.join("  ");
write_line(
    area,
    buf,
    area.y,
    &header,
    Style::default()
        .fg(self.theme().header)
        .add_modifier(Modifier::BOLD),
);
```

Leave the rest of the `render` method intact.

- [ ] **Step 2: Verify no more header**

```bash
cargo test -p neo-tui --test app_shell app_shell_renders_context_window 2>&1 | tail -30
```

Expected: tests run; header-specific assertions removed in Task 9 should now pass.

- [ ] **Step 3: Commit**

```bash
git add crates/neo-tui/src/components.rs
git commit -m "feat(tui): remove persistent header render block"
```

---

### Task 8: Push startup banner and pass workspace root from interactive mode

**Files:**
- Modify: `crates/neo-agent/src/modes/interactive.rs:358-389` (`new_with_turn_driver`)
- Modify: `crates/neo-agent/src/modes/interactive.rs:408-427` (`apply_startup_options`)

- [ ] **Step 1: Accept workspace root in constructor**

```rust
pub fn new_with_turn_driver(
    title: impl Into<String>,
    session_label: impl Into<String>,
    model_label: impl Into<String>,
    workspace_root: impl Into<PathBuf>,
    run_turn: TurnDriver,
    catalogs: PickerCatalogs,
    load_session: SessionLoader,
    fork_session: SessionForker,
) -> Self {
    Self {
        app: NeoTuiApp::new(title, session_label, model_label, workspace_root),
        // ... rest unchanged
    }
}
```

- [ ] **Step 2: Update `new()` to forward workspace root**

Find the call to `Self::new_with_turn_driver` in `new()` and add `PathBuf::from(".")` or the actual workspace root if available. If the original `new()` does not have workspace root, pass `std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."))`.

- [ ] **Step 3: Push banner on startup**

In `apply_startup_options`, replace the notice push with a banner push:

```rust
self.app
    .transcript_mut()
    .push(neo_tui::TranscriptItem::banner(
        format!("Welcome to {}", self.app.title()),
        self.app.session_label().to_owned(),
        self.app.model_label().to_owned(),
        self.app.workspace_root().clone(),
    ));

if options.verbose_startup {
    self.app
        .transcript_mut()
        .push(neo_tui::TranscriptItem::notice(
            startup_notices(config).join("\n"),
        ));
}
```

- [ ] **Step 4: Find callers of `new_with_turn_driver` and `new()`**

```bash
rg 'InteractiveController::new\(|InteractiveController::new_with_turn_driver\(' crates/neo-agent/src
```

Update each call site to pass the workspace root. The workspace root is typically available as `config.project_dir` or `env::current_dir()`.

- [ ] **Step 5: Compile**

```bash
cargo check -p neo-agent 2>&1 | tail -40
```

Expected: no errors.

- [ ] **Step 6: Commit**

```bash
git add crates/neo-agent/src/modes/interactive.rs
git commit -m "feat(neo-agent): push startup banner and pass workspace root to TUI"
```

---

### Task 9: Update tests for new layout

**Files:**
- Modify: `crates/neo-tui/tests/app_shell.rs`

- [ ] **Step 1: Update existing assertions**

Replace assertions that expect the old header/footer strings. Examples:

```rust
// Old test: app_shell_renders_context_window_and_working_status
// Replace header assertion with footer assertion.
assert!(lines.iter().any(|line| line.contains("ctx 12k/200k")));
assert!(lines.iter().any(|line| line.contains("● working")));
// Add: assert no persistent top header row
let first_body_row = lines.first().expect("terminal has height");
assert!(!first_body_row.contains("session:"));
assert!(!first_body_row.contains("model:"));
```

- [ ] **Step 2: Add banner test**

```rust
#[test]
fn app_shell_renders_startup_banner() {
    let mut app = NeoTuiApp::new("neo", "session-a", "openai/gpt-4.1", "/tmp/neo-ws");
    app.transcript_mut().push(neo_tui::TranscriptItem::banner(
        "Welcome to neo",
        "session-a",
        "openai/gpt-4.1",
        "/tmp/neo-ws",
    ));

    let lines = render_app(80, 12, &app);
    assert!(lines.iter().any(|line| line.contains("Welcome to neo")));
    assert!(lines.iter().any(|line| line.contains("session-a")));
    assert!(lines.iter().any(|line| line.contains("openai/gpt-4.1")));
}
```

- [ ] **Step 3: Add context threshold color test**

```rust
#[test]
fn app_shell_context_color_changes_by_threshold() {
    use ratatui::style::Color;

    let mut app = NeoTuiApp::new("neo", "session-a", "openai/gpt-4.1", "/tmp/neo-ws");
    let theme = app.theme();

    app.set_context_window(Some(ContextWindow::new(100).with_used_tokens(50)));
    assert_eq!(app.context_color(), theme.footer_context_ok);

    app.set_context_window(Some(ContextWindow::new(100).with_used_tokens(75)));
    assert_eq!(app.context_color(), theme.footer_context_warn);

    app.set_context_window(Some(ContextWindow::new(100).with_used_tokens(95)));
    assert_eq!(app.context_color(), theme.footer_context_critical);
}
```

- [ ] **Step 4: Add footer line count test**

```rust
#[test]
fn app_shell_footer_has_two_lines_when_tall() {
    let mut app = NeoTuiApp::new("neo", "session-a", "openai/gpt-4.1", "/tmp/neo-ws");
    app.set_context_window(Some(ContextWindow::new(200_000).with_used_tokens(12_345)));

    let lines = render_app(100, 12, &app);
    // Last two rows are footer; check both contain expected content.
    assert!(lines[lines.len() - 2].contains("session-a") || lines[lines.len() - 2].contains("neo"));
    assert!(lines[lines.len() - 1].contains("enter send"));
    assert!(lines[lines.len() - 1].contains("ctx 12k/200k"));
}
```

- [ ] **Step 5: Run tests**

```bash
cargo test -p neo-tui --test app_shell 2>&1 | tail -50
```

Expected: all tests pass.

- [ ] **Step 6: Commit**

```bash
git add crates/neo-tui/tests/app_shell.rs
git commit -m "test(tui): update app_shell tests for banner and two-line footer"
```

---

### Task 10: Run full workspace checks

**Files:**
- All modified files

- [ ] **Step 1: Format**

```bash
cargo fmt --all
```

- [ ] **Step 2: Run clippy**

```bash
cargo clippy -p neo-tui -p neo-agent --all-targets --all-features -- -D warnings 2>&1 | tail -60
```

Expected: no warnings.

- [ ] **Step 3: Run tests**

```bash
cargo test -p neo-tui -p neo-agent --all-features 2>&1 | tail -60
```

Expected: all tests pass.

- [ ] **Step 4: Manual smoke**

```bash
cargo build -p neo-agent
cargo run -p neo-agent -- print "hello"
```

Then run the interactive TUI (requires API key or fake provider):

```bash
# If a fake provider is configured:
cargo run -p neo-agent --
```

Visually verify:
- No top header.
- Startup banner appears in transcript.
- Footer has two lines.
- Context usage is right-aligned.

- [ ] **Step 5: Commit formatting fixes**

```bash
git add -A
git commit -m "style: cargo fmt"
```

---

## Self-Review

### Spec Coverage

| Spec Section | Implementing Task |
|---|---|
| Remove persistent header | Task 5, Task 7 |
| Startup banner | Task 2, Task 8 |
| Two-line footer | Task 5, Task 6 |
| Semantic colors | Task 3, Task 4 |
| Workspace root in footer | Task 1, Task 8 |
| Context threshold coloring | Task 6, Task 9 |
| Responsive footer | Task 5, Task 6 |
| Theme JSON parsing | Task 4 |
| Tests | Task 9, Task 10 |

### Caveat-word Scan

- No unresolved marker strings or "implement later" phrasing remain.
- If the plan intentionally names caveat wording for a task or fixture, keep it paired with an `xtask-parity: allow` comment or rewrite the wording to be descriptive rather than prospective.
- Every code step includes concrete code or exact commands.
- Every task ends with a commit command.

### Type Consistency

- `NeoTuiApp::new` signature updated consistently across `app.rs`, `tests/app_shell.rs`, and `interactive.rs`.
- `TuiTheme` fields added in Task 3 match `ThemeColors` fields added in Task 4.
- `TranscriptItem::Banner` constructor signature matches the enum variant.

### Known Gaps

1. `permission_badge()` in Task 6 now maps the stored `PermissionDecision` to `[allow]`, `[ask]`, or `[deny]`, so the footer color can stay aligned with the active policy.
2. Git branch/change counts are intentionally deferred to future work per the spec.
