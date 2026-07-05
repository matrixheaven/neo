//! Raw terminal input parser — ports pi-tui's `stdin-buffer.ts` and `keys.ts`.
//!
//! Reads raw stdin bytes, buffers them into complete ANSI escape sequences
//! (like pi-tui's `StdinBuffer`), and provides key matching / parsing logic
//! (like pi-tui's `matchesKey` / `parseKey`).
//!
//! By reading raw bytes instead of relying on crossterm's event parsing, the
//! application has full control over input semantics. This fixes issues like
//! Ctrl+V being intercepted by the terminal as a system paste on macOS.

use std::sync::OnceLock;
use std::sync::atomic::{AtomicBool, Ordering};

use regex::Regex;

// ===========================================================================
// Constants
// ===========================================================================

const ESC: &str = "\x1b";
const BRACKETED_PASTE_START: &str = "\x1b[200~";
const BRACKETED_PASTE_END: &str = "\x1b[201~";

const MOD_SHIFT: u32 = 1;
const MOD_ALT: u32 = 2;
const MOD_CTRL: u32 = 4;
const MOD_SUPER: u32 = 8;
const LOCK_MASK: u32 = 64 + 128; // Caps Lock + Num Lock

const CP_ESCAPE: i32 = 27;
const CP_TAB: i32 = 9;
const CP_ENTER: i32 = 13;
const CP_SPACE: i32 = 32;
const CP_BACKSPACE: i32 = 127;
const CP_KP_ENTER: i32 = 57414;

const CP_UP: i32 = -1;
const CP_DOWN: i32 = -2;
const CP_RIGHT: i32 = -3;
const CP_LEFT: i32 = -4;

const CP_DELETE: i32 = -10;
const CP_INSERT: i32 = -11;
const CP_PAGE_UP: i32 = -12;
const CP_PAGE_DOWN: i32 = -13;
const CP_HOME: i32 = -14;
const CP_END: i32 = -15;

// ===========================================================================
// Global Kitty Protocol State
// ===========================================================================

static KITTY_PROTOCOL: AtomicBool = AtomicBool::new(false);

/// Set the global Kitty keyboard protocol state.
pub fn set_kitty_protocol_active(active: bool) {
    KITTY_PROTOCOL.store(active, Ordering::Relaxed);
}

/// Query whether Kitty keyboard protocol is currently active.
#[must_use]
pub fn is_kitty_protocol_active() -> bool {
    KITTY_PROTOCOL.load(Ordering::Relaxed)
}

// ===========================================================================
// RawEvent
// ===========================================================================

/// A complete unit of input parsed from raw stdin bytes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RawEvent {
    /// A complete ANSI sequence string (to be matched against keybindings).
    Key(String),
    /// Bracketed paste content.
    Paste(String),
}

// ===========================================================================
// Sequence completeness state machine (from stdin-buffer.ts)
// ===========================================================================

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SequenceStatus {
    Complete,
    Incomplete,
    NotEscape,
}

fn is_complete_sequence(data: &str) -> SequenceStatus {
    if !data.starts_with(ESC) {
        return SequenceStatus::NotEscape;
    }
    if data.len() == 1 {
        return SequenceStatus::Incomplete;
    }

    let after_esc = &data[ESC.len()..];

    // Meta + escape sequence (for example Alt+Up in terminals that encode it
    // as ESC followed by the normal arrow sequence).
    if after_esc.starts_with(ESC) {
        return is_complete_sequence(after_esc);
    }

    // CSI sequences: ESC [
    if after_esc.starts_with('[') {
        // Old-style mouse: ESC[M + 3 bytes = 6 total
        if after_esc.starts_with("[M") {
            return if data.len() >= 6 {
                SequenceStatus::Complete
            } else {
                SequenceStatus::Incomplete
            };
        }
        return is_complete_csi_sequence(data);
    }

    // OSC sequences: ESC ]
    if after_esc.starts_with(']') {
        return is_complete_osc_sequence(data);
    }

    // DCS sequences: ESC P
    if after_esc.starts_with('P') {
        return is_complete_dcs_sequence(data);
    }

    // APC sequences: ESC _
    if after_esc.starts_with('_') {
        return is_complete_apc_sequence(data);
    }

    // SS3 sequences: ESC O — followed by a single character
    if after_esc.starts_with('O') {
        return if after_esc.len() >= 2 {
            SequenceStatus::Complete
        } else {
            SequenceStatus::Incomplete
        };
    }

    // Meta key sequences: ESC followed by a single character
    if after_esc.chars().count() == 1 {
        return SequenceStatus::Complete;
    }

    // Unknown escape sequence — treat as complete
    SequenceStatus::Complete
}

fn is_complete_csi_sequence(data: &str) -> SequenceStatus {
    if !data.starts_with("\x1b[") {
        return SequenceStatus::Complete;
    }
    if data.len() < 3 {
        return SequenceStatus::Incomplete;
    }

    let payload = &data[2..];
    let last_char = payload.chars().last().unwrap_or('\0');
    let last_code = last_char as u32;

    if (0x40..=0x7e).contains(&last_code) {
        // SGR mouse sequences: ESC[<B;X;Ym or ESC[<B;X;YM
        if payload.starts_with('<') {
            // Must match <digits;digits;digits[Mm]
            if is_sgr_mouse_payload(payload) {
                return SequenceStatus::Complete;
            }
            return SequenceStatus::Incomplete;
        }
        return SequenceStatus::Complete;
    }
    SequenceStatus::Incomplete
}

fn is_sgr_mouse_payload(payload: &str) -> bool {
    // Payload starts with '<', must be <digits;digits;digits[Mm]
    let body = &payload[1..]; // skip '<'
    let body = body.strip_suffix(['M', 'm']).unwrap_or(body);
    let parts: Vec<&str> = body.split(';').collect();
    parts.len() == 3
        && parts
            .iter()
            .all(|p| !p.is_empty() && p.chars().all(|c| c.is_ascii_digit()))
}

fn is_complete_osc_sequence(data: &str) -> SequenceStatus {
    if !data.starts_with("\x1b]") {
        return SequenceStatus::Complete;
    }
    if data.ends_with("\x1b\\") || data.ends_with('\x07') {
        return SequenceStatus::Complete;
    }
    SequenceStatus::Incomplete
}

fn is_complete_dcs_sequence(data: &str) -> SequenceStatus {
    if !data.starts_with("\x1bP") {
        return SequenceStatus::Complete;
    }
    if data.ends_with("\x1b\\") {
        return SequenceStatus::Complete;
    }
    SequenceStatus::Incomplete
}

fn is_complete_apc_sequence(data: &str) -> SequenceStatus {
    if !data.starts_with("\x1b_") {
        return SequenceStatus::Complete;
    }
    if data.ends_with("\x1b\\") {
        return SequenceStatus::Complete;
    }
    SequenceStatus::Incomplete
}

/// Split accumulated buffer into complete sequences.
/// Returns (sequences, remainder).
fn extract_complete_sequences(buffer: &str) -> (Vec<String>, String) {
    let mut sequences = Vec::new();
    let chars: Vec<char> = buffer.chars().collect();
    let mut pos = 0;

    while pos < chars.len() {
        let remaining: String = chars[pos..].iter().collect();

        if remaining.starts_with(ESC) {
            // Find the end of this escape sequence
            let mut seq_end = 1;
            let mut found = false;

            while seq_end <= remaining.len() {
                let candidate: String = remaining[..seq_end].to_owned();
                match is_complete_sequence(&candidate) {
                    SequenceStatus::Complete => {
                        // WezTerm ESC ESC workaround
                        if candidate == "\x1b\x1b" {
                            let next_char = remaining.chars().nth(seq_end);
                            if matches!(next_char, Some('[' | ']' | 'O' | 'P' | '_')) {
                                sequences.push(ESC.to_owned());
                                pos += 1;
                                found = true;
                                break;
                            }
                        }
                        sequences.push(candidate);
                        pos += seq_end;
                        found = true;
                        break;
                    }
                    SequenceStatus::Incomplete => {
                        seq_end += 1;
                    }
                    SequenceStatus::NotEscape => {
                        sequences.push(candidate);
                        pos += seq_end;
                        found = true;
                        break;
                    }
                }
            }

            if !found {
                // Ran out of characters — incomplete sequence
                return (sequences, remaining);
            }
        } else {
            // Not an escape sequence — take a single character
            let ch = chars[pos];
            sequences.push(ch.to_string());
            pos += 1;
        }
    }

    (sequences, String::new())
}

// ===========================================================================
// Kitty printable codepoint parsing
// ===========================================================================

fn parse_unmodified_kitty_printable_codepoint(sequence: &str) -> Option<i32> {
    let re = csi_u_simple_regex();
    let caps = re.captures(sequence)?;
    let codepoint: i32 = caps.get(1)?.as_str().parse().ok()?;
    (codepoint >= 32).then_some(codepoint)
}

fn csi_u_simple_regex() -> &'static Regex {
    static R: OnceLock<Regex> = OnceLock::new();
    R.get_or_init(|| Regex::new(r"^\x1b\[(\d+)(?::\d*)?(?::\d+)?u$").expect("valid regex"))
}

// ===========================================================================
// RawInputParser (StdinBuffer port)
// ===========================================================================

/// Buffers raw stdin bytes and emits complete sequences.
///
/// Equivalent to pi-tui's `StdinBuffer`. Handles:
/// - Bracketed paste detection (ESC[200~ ... ESC[201~)
/// - Splitting concatenated escape sequences into individual complete sequences
/// - `WezTerm` ESC ESC workaround
/// - Kitty printable dedup logic
#[derive(Debug, Clone)]
pub struct RawInputParser {
    buffer: String,
    pending_utf8: Vec<u8>,
    paste_mode: bool,
    paste_buffer: String,
    pending_kitty_printable_codepoint: Option<i32>,
}

impl Default for RawInputParser {
    fn default() -> Self {
        Self::new()
    }
}

impl RawInputParser {
    #[must_use]
    pub fn new() -> Self {
        Self {
            buffer: String::new(),
            pending_utf8: Vec::new(),
            paste_mode: false,
            paste_buffer: String::new(),
            pending_kitty_printable_codepoint: None,
        }
    }

    /// Feed raw stdin bytes and return complete events.
    pub fn feed_bytes(&mut self, data: &[u8]) -> Vec<RawEvent> {
        let str_data = self.decode_input_bytes(data);

        if str_data.is_empty() && self.buffer.is_empty() {
            return Vec::new();
        }

        self.buffer.push_str(&str_data);
        let mut events = Vec::new();

        if self.paste_mode {
            self.paste_buffer
                .push_str(&std::mem::take(&mut self.buffer));

            if let Some(end_index) = self.paste_buffer.find(BRACKETED_PASTE_END) {
                let pasted_content = self.paste_buffer[..end_index].to_owned();
                let remaining =
                    self.paste_buffer[end_index + BRACKETED_PASTE_END.len()..].to_owned();

                self.paste_mode = false;
                self.paste_buffer.clear();
                self.pending_kitty_printable_codepoint = None;

                events.push(RawEvent::Paste(pasted_content));

                if !remaining.is_empty() {
                    events.extend(self.process_internal(&remaining));
                }
            }
            return events;
        }

        let buf = std::mem::take(&mut self.buffer);
        events.extend(self.process_internal(&buf));
        events
    }

    fn decode_input_bytes(&mut self, data: &[u8]) -> String {
        if self.pending_utf8.is_empty() && data.len() == 1 && is_meta_continuation_byte(data[0]) {
            return meta_byte_to_escape_sequence(data[0]);
        }

        let mut bytes = std::mem::take(&mut self.pending_utf8);
        bytes.extend_from_slice(data);
        let mut output = String::new();
        let mut offset = 0;

        while offset < bytes.len() {
            match std::str::from_utf8(&bytes[offset..]) {
                Ok(valid) => {
                    output.push_str(valid);
                    break;
                }
                Err(error) => {
                    let valid_up_to = error.valid_up_to();
                    if valid_up_to > 0 {
                        let valid_end = offset + valid_up_to;
                        output.push_str(
                            std::str::from_utf8(&bytes[offset..valid_end])
                                .expect("valid_up_to must split at a UTF-8 boundary"),
                        );
                        offset = valid_end;
                    }

                    let Some(error_len) = error.error_len() else {
                        self.pending_utf8.extend_from_slice(&bytes[offset..]);
                        break;
                    };

                    let invalid_end = offset + error_len;
                    if error_len == 1 && is_meta_continuation_byte(bytes[offset]) {
                        output.push_str(&meta_byte_to_escape_sequence(bytes[offset]));
                    } else {
                        output.push('\u{fffd}');
                    }
                    offset = invalid_end;
                }
            }
        }

        output
    }

    fn process_internal(&mut self, data: &str) -> Vec<RawEvent> {
        let mut events = Vec::new();
        data.clone_into(&mut self.buffer);

        // Check for bracketed paste start
        if let Some(start_index) = self.buffer.find(BRACKETED_PASTE_START) {
            if start_index > 0 {
                let before_paste = &self.buffer[..start_index];
                let (sequences, _) = extract_complete_sequences(before_paste);
                for seq in sequences {
                    self.emit_data_sequence(&seq, &mut events);
                }
            }

            self.pending_kitty_printable_codepoint = None;
            self.buffer = self.buffer[start_index + BRACKETED_PASTE_START.len()..].to_owned();
            self.paste_mode = true;
            self.paste_buffer = std::mem::take(&mut self.buffer);

            if let Some(end_index) = self.paste_buffer.find(BRACKETED_PASTE_END) {
                let pasted_content = self.paste_buffer[..end_index].to_owned();
                let remaining =
                    self.paste_buffer[end_index + BRACKETED_PASTE_END.len()..].to_owned();

                self.paste_mode = false;
                self.paste_buffer.clear();
                self.pending_kitty_printable_codepoint = None;

                events.push(RawEvent::Paste(pasted_content));

                if !remaining.is_empty() {
                    events.extend(self.process_internal(&remaining));
                }
            }
            return events;
        }

        let (sequences, remainder) = extract_complete_sequences(&self.buffer);
        self.buffer = remainder;

        for seq in sequences {
            self.emit_data_sequence(&seq, &mut events);
        }

        events
    }

    fn emit_data_sequence(&mut self, sequence: &str, events: &mut Vec<RawEvent>) {
        // Kitty printable dedup
        let raw_codepoint = if sequence.chars().count() == 1 {
            sequence.chars().next().map(|c| c as i32)
        } else {
            None
        };

        if let Some(cp) = raw_codepoint
            && Some(cp) == self.pending_kitty_printable_codepoint
        {
            self.pending_kitty_printable_codepoint = None;
            return;
        }

        self.pending_kitty_printable_codepoint =
            parse_unmodified_kitty_printable_codepoint(sequence);
        events.push(RawEvent::Key(sequence.to_owned()));
    }

    /// Force-flush any buffered incomplete sequences.
    pub fn flush(&mut self) -> Vec<RawEvent> {
        if !self.pending_utf8.is_empty() {
            let bytes = std::mem::take(&mut self.pending_utf8);
            self.buffer.push_str(&String::from_utf8_lossy(&bytes));
        }
        if self.buffer.is_empty() {
            return Vec::new();
        }
        let seq = std::mem::take(&mut self.buffer);
        self.pending_kitty_printable_codepoint = None;
        vec![RawEvent::Key(seq)]
    }
}

fn is_meta_continuation_byte(byte: u8) -> bool {
    (0x80..0xc0).contains(&byte)
}

fn meta_byte_to_escape_sequence(byte: u8) -> String {
    let key = byte - 128;
    format!("\x1b{}", key as char)
}

// ===========================================================================
// Key parsing (from keys.ts)
// ===========================================================================

/// Result of parsing a Kitty CSI-u or similar sequence.
#[derive(Debug, Clone)]
struct ParsedKitty {
    codepoint: i32,
    base_layout_key: Option<i32>,
    modifier: u32,
}

#[derive(Debug, Clone)]
struct ParsedModifyOtherKeys {
    codepoint: i32,
    modifier: u32,
}

fn csi_u_regex() -> &'static Regex {
    static R: OnceLock<Regex> = OnceLock::new();
    R.get_or_init(|| {
        Regex::new(r"^\x1b\[(\d+)(?::(\d*))?(?::(\d+))?(?:;(\d+))?(?::(\d+))?u$")
            .expect("valid regex")
    })
}

fn arrow_mod_regex() -> &'static Regex {
    static R: OnceLock<Regex> = OnceLock::new();
    R.get_or_init(|| Regex::new(r"^\x1b\[1;(\d+)(?::(\d+))?([ABCD])$").expect("valid regex"))
}

fn func_regex() -> &'static Regex {
    static R: OnceLock<Regex> = OnceLock::new();
    R.get_or_init(|| Regex::new(r"^\x1b\[(\d+)(?:;(\d+))?(?::(\d+))?~$").expect("valid regex"))
}

fn home_end_mod_regex() -> &'static Regex {
    static R: OnceLock<Regex> = OnceLock::new();
    R.get_or_init(|| Regex::new(r"^\x1b\[1;(\d+)(?::(\d+))?([HF])$").expect("valid regex"))
}

fn modify_other_keys_regex() -> &'static Regex {
    static R: OnceLock<Regex> = OnceLock::new();
    R.get_or_init(|| Regex::new(r"^\x1b\[27;(\d+);(\d+)~$").expect("valid regex"))
}

fn parse_kitty_sequence(data: &str) -> Option<ParsedKitty> {
    // CSI-u format
    if let Some(caps) = csi_u_regex().captures(data) {
        let codepoint: i32 = caps.get(1)?.as_str().parse().ok()?;
        let base_layout_key = caps.get(3).and_then(|m| m.as_str().parse().ok());
        let mod_value: u32 = caps
            .get(4)
            .and_then(|m| m.as_str().parse().ok())
            .unwrap_or(1);
        return Some(ParsedKitty {
            codepoint,
            base_layout_key,
            modifier: mod_value.saturating_sub(1),
        });
    }

    // Arrow keys with modifier
    if let Some(caps) = arrow_mod_regex().captures(data) {
        let mod_value: u32 = caps.get(1)?.as_str().parse().ok()?;
        let letter = caps.get(3)?.as_str();
        let codepoint = match letter {
            "A" => CP_UP,
            "B" => CP_DOWN,
            "C" => CP_RIGHT,
            "D" => CP_LEFT,
            _ => return None,
        };
        return Some(ParsedKitty {
            codepoint,
            base_layout_key: None,
            modifier: mod_value.saturating_sub(1),
        });
    }

    // Functional keys: \x1b[<num>[;<mod>[:<event>]]~
    if let Some(caps) = func_regex().captures(data) {
        let key_num: u32 = caps.get(1)?.as_str().parse().ok()?;
        let mod_value: u32 = caps
            .get(2)
            .and_then(|m| m.as_str().parse().ok())
            .unwrap_or(1);
        let codepoint = match key_num {
            2 => CP_INSERT,
            3 => CP_DELETE,
            5 => CP_PAGE_UP,
            6 => CP_PAGE_DOWN,
            7 => CP_HOME,
            8 => CP_END,
            _ => return None,
        };
        return Some(ParsedKitty {
            codepoint,
            base_layout_key: None,
            modifier: mod_value.saturating_sub(1),
        });
    }

    // Home/End with modifier
    if let Some(caps) = home_end_mod_regex().captures(data) {
        let mod_value: u32 = caps.get(1)?.as_str().parse().ok()?;
        let letter = caps.get(3)?.as_str();
        let codepoint = if letter == "H" { CP_HOME } else { CP_END };
        return Some(ParsedKitty {
            codepoint,
            base_layout_key: None,
            modifier: mod_value.saturating_sub(1),
        });
    }

    None
}

fn parse_modify_other_keys_sequence(data: &str) -> Option<ParsedModifyOtherKeys> {
    let caps = modify_other_keys_regex().captures(data)?;
    let mod_value: u32 = caps.get(1)?.as_str().parse().ok()?;
    let codepoint: i32 = caps.get(2)?.as_str().parse().ok()?;
    Some(ParsedModifyOtherKeys {
        codepoint,
        modifier: mod_value.saturating_sub(1),
    })
}

// ===========================================================================
// Normalization helpers
// ===========================================================================

fn kitty_functional_equivalent(codepoint: i32) -> i32 {
    match codepoint {
        57399 => 48, // KP_0 -> 0
        57400 => 49, // KP_1 -> 1
        57401 => 50, // KP_2 -> 2
        57402 => 51, // KP_3 -> 3
        57403 => 52, // KP_4 -> 4
        57404 => 53, // KP_5 -> 5
        57405 => 54, // KP_6 -> 6
        57406 => 55, // KP_7 -> 7
        57407 => 56, // KP_8 -> 8
        57408 => 57, // KP_9 -> 9
        57409 => 46, // KP_DECIMAL -> .
        57410 => 47, // KP_DIVIDE -> /
        57411 => 42, // KP_MULTIPLY -> *
        57412 => 45, // KP_SUBTRACT -> -
        57413 => 43, // KP_ADD -> +
        57415 => 61, // KP_EQUAL -> =
        57416 => 44, // KP_SEPARATOR -> ,
        57417 => CP_LEFT,
        57418 => CP_RIGHT,
        57419 => CP_UP,
        57420 => CP_DOWN,
        57421 => CP_PAGE_UP,
        57422 => CP_PAGE_DOWN,
        57423 => CP_HOME,
        57424 => CP_END,
        57425 => CP_INSERT,
        57426 => CP_DELETE,
        _ => codepoint,
    }
}

fn normalize_shifted_letter_identity(codepoint: i32, modifier: u32) -> i32 {
    let effective = modifier & !LOCK_MASK;
    if (effective & MOD_SHIFT) != 0 && (65..=90).contains(&codepoint) {
        return codepoint + 32;
    }
    codepoint
}

fn is_symbol_key(c: char) -> bool {
    matches!(
        c,
        '`' | '-'
            | '='
            | '['
            | ']'
            | '\\'
            | ';'
            | '\''
            | ','
            | '.'
            | '/'
            | '!'
            | '@'
            | '#'
            | '$'
            | '%'
            | '^'
            | '&'
            | '*'
            | '('
            | ')'
            | '_'
            | '+'
            | '|'
            | '~'
            | '{'
            | '}'
            | ':'
            | '<'
            | '>'
            | '?'
    )
}

/// Compute the raw control character for a key (code & 0x1f).
fn raw_ctrl_char(key: &str) -> Option<char> {
    let c = key.chars().next()?;
    let code = c as u32;
    if (97..=122).contains(&code) || c == '[' || c == '\\' || c == ']' || c == '_' {
        Some(char::from_u32(code & 0x1f)?)
    } else if c == '-' {
        Some('\x1f') // same as Ctrl+_
    } else {
        None
    }
}

// ===========================================================================
// Formatting helpers
// ===========================================================================

fn format_key_name_with_modifiers(key_name: &str, modifier: u32) -> Option<String> {
    let effective = modifier & !LOCK_MASK;
    let supported = MOD_SHIFT | MOD_CTRL | MOD_ALT | MOD_SUPER;
    if (effective & !supported) != 0 {
        return None;
    }
    let mut mods = Vec::new();
    if effective & MOD_SHIFT != 0 {
        mods.push("shift");
    }
    if effective & MOD_CTRL != 0 {
        mods.push("ctrl");
    }
    if effective & MOD_ALT != 0 {
        mods.push("alt");
    }
    if effective & MOD_SUPER != 0 {
        mods.push("super");
    }
    if mods.is_empty() {
        Some(key_name.to_owned())
    } else {
        Some(format!("{}+{}", mods.join("+"), key_name))
    }
}

fn format_parsed_key(
    codepoint: i32,
    modifier: u32,
    base_layout_key: Option<i32>,
) -> Option<String> {
    let normalized = kitty_functional_equivalent(codepoint);
    let identity = normalize_shifted_letter_identity(normalized, modifier);

    let is_latin_letter = (97..=122).contains(&identity);
    let is_digit = (48..=57).contains(&identity);
    let is_known_symbol = char::from_u32(identity.cast_unsigned()).is_some_and(is_symbol_key);

    let effective_cp = if is_latin_letter || is_digit || is_known_symbol {
        identity
    } else {
        base_layout_key.unwrap_or(identity)
    };

    let key_name = codepoint_to_key_name(effective_cp)?;
    format_key_name_with_modifiers(&key_name, modifier)
}

fn codepoint_to_key_name(cp: i32) -> Option<String> {
    if cp == CP_ESCAPE {
        return Some("escape".to_owned());
    }
    if cp == CP_TAB {
        return Some("tab".to_owned());
    }
    if cp == CP_ENTER || cp == CP_KP_ENTER {
        return Some("enter".to_owned());
    }
    if cp == CP_SPACE {
        return Some("space".to_owned());
    }
    if cp == CP_BACKSPACE {
        return Some("backspace".to_owned());
    }
    if cp == CP_DELETE {
        return Some("delete".to_owned());
    }
    if cp == CP_INSERT {
        return Some("insert".to_owned());
    }
    if cp == CP_HOME {
        return Some("home".to_owned());
    }
    if cp == CP_END {
        return Some("end".to_owned());
    }
    if cp == CP_PAGE_UP {
        return Some("pageUp".to_owned());
    }
    if cp == CP_PAGE_DOWN {
        return Some("pageDown".to_owned());
    }
    if cp == CP_UP {
        return Some("up".to_owned());
    }
    if cp == CP_DOWN {
        return Some("down".to_owned());
    }
    if cp == CP_LEFT {
        return Some("left".to_owned());
    }
    if cp == CP_RIGHT {
        return Some("right".to_owned());
    }
    if (48..=57).contains(&cp) {
        return Some(char::from_u32(cp.cast_unsigned())?.to_string());
    }
    if (97..=122).contains(&cp) {
        return Some(char::from_u32(cp.cast_unsigned())?.to_string());
    }
    if let Some(c) = char::from_u32(cp.cast_unsigned())
        && is_symbol_key(c)
    {
        return Some(c.to_string());
    }
    None
}

// ===========================================================================
// Legacy sequence lookups
// ===========================================================================

#[allow(clippy::match_same_arms)]
fn legacy_sequence_key_id(data: &str) -> Option<&'static str> {
    match data {
        "\x1bOA" => Some("up"),
        "\x1bOB" => Some("down"),
        "\x1bOC" => Some("right"),
        "\x1bOD" => Some("left"),
        "\x1bOH" => Some("home"),
        "\x1bOF" => Some("end"),
        "\x1b[E" => Some("clear"),
        "\x1bOE" => Some("clear"),
        "\x1bOe" => Some("ctrl+clear"),
        "\x1b[e" => Some("shift+clear"),
        "\x1b[2~" => Some("insert"),
        "\x1b[2$" => Some("shift+insert"),
        "\x1b[2^" => Some("ctrl+insert"),
        "\x1b[3$" => Some("shift+delete"),
        "\x1b[3^" => Some("ctrl+delete"),
        "\x1b[[5~" => Some("pageUp"),
        "\x1b[[6~" => Some("pageDown"),
        "\x1b[a" => Some("shift+up"),
        "\x1b[b" => Some("shift+down"),
        "\x1b[c" => Some("shift+right"),
        "\x1b[d" => Some("shift+left"),
        "\x1bOa" => Some("ctrl+up"),
        "\x1bOb" => Some("ctrl+down"),
        "\x1bOc" => Some("ctrl+right"),
        "\x1bOd" => Some("ctrl+left"),
        "\x1b[5$" => Some("shift+pageUp"),
        "\x1b[6$" => Some("shift+pageDown"),
        "\x1b[7$" => Some("shift+home"),
        "\x1b[8$" => Some("shift+end"),
        "\x1b[5^" => Some("ctrl+pageUp"),
        "\x1b[6^" => Some("ctrl+pageDown"),
        "\x1b[7^" => Some("ctrl+home"),
        "\x1b[8^" => Some("ctrl+end"),
        "\x1bOP" => Some("f1"),
        "\x1bOQ" => Some("f2"),
        "\x1bOR" => Some("f3"),
        "\x1bOS" => Some("f4"),
        "\x1b[11~" => Some("f1"),
        "\x1b[12~" => Some("f2"),
        "\x1b[13~" => Some("f3"),
        "\x1b[14~" => Some("f4"),
        "\x1b[[A" => Some("f1"),
        "\x1b[[B" => Some("f2"),
        "\x1b[[C" => Some("f3"),
        "\x1b[[D" => Some("f4"),
        "\x1b[[E" => Some("f5"),
        "\x1b[15~" => Some("f5"),
        "\x1b[17~" => Some("f6"),
        "\x1b[18~" => Some("f7"),
        "\x1b[19~" => Some("f8"),
        "\x1b[20~" => Some("f9"),
        "\x1b[21~" => Some("f10"),
        "\x1b[23~" => Some("f11"),
        "\x1b[24~" => Some("f12"),
        "\x1bb" => Some("alt+left"),
        "\x1bf" => Some("alt+right"),
        "\x1bp" => Some("alt+up"),
        "\x1bn" => Some("alt+down"),
        "\x1b\x1b[A" | "\x1b\x1bOA" => Some("alt+up"),
        "\x1b\x1b[B" | "\x1b\x1bOB" => Some("alt+down"),
        "\x1b\x1b[D" | "\x1b\x1bOD" => Some("alt+left"),
        "\x1b\x1b[C" | "\x1b\x1bOC" => Some("alt+right"),
        _ => None,
    }
}

fn legacy_key_sequences(key: &str) -> &'static [&'static str] {
    match key {
        "up" => &["\x1b[A", "\x1bOA"],
        "down" => &["\x1b[B", "\x1bOB"],
        "right" => &["\x1b[C", "\x1bOC"],
        "left" => &["\x1b[D", "\x1bOD"],
        "home" => &["\x1b[H", "\x1bOH", "\x1b[1~", "\x1b[7~"],
        "end" => &["\x1b[F", "\x1bOF", "\x1b[4~", "\x1b[8~"],
        "insert" => &["\x1b[2~"],
        "delete" => &["\x1b[3~"],
        "pageUp" => &["\x1b[5~", "\x1b[[5~"],
        "pageDown" => &["\x1b[6~", "\x1b[[6~"],
        "clear" => &["\x1b[E", "\x1bOE"],
        "f1" => &["\x1bOP", "\x1b[11~", "\x1b[[A"],
        "f2" => &["\x1bOQ", "\x1b[12~", "\x1b[[B"],
        "f3" => &["\x1bOR", "\x1b[13~", "\x1b[[C"],
        "f4" => &["\x1bOS", "\x1b[14~", "\x1b[[D"],
        "f5" => &["\x1b[15~", "\x1b[[E"],
        "f6" => &["\x1b[17~"],
        "f7" => &["\x1b[18~"],
        "f8" => &["\x1b[19~"],
        "f9" => &["\x1b[20~"],
        "f10" => &["\x1b[21~"],
        "f11" => &["\x1b[23~"],
        "f12" => &["\x1b[24~"],
        _ => &[],
    }
}

fn legacy_shift_sequences(key: &str) -> &'static [&'static str] {
    match key {
        "up" => &["\x1b[a"],
        "down" => &["\x1b[b"],
        "right" => &["\x1b[c"],
        "left" => &["\x1b[d"],
        "clear" => &["\x1b[e"],
        "insert" => &["\x1b[2$"],
        "delete" => &["\x1b[3$"],
        "pageUp" => &["\x1b[5$"],
        "pageDown" => &["\x1b[6$"],
        "home" => &["\x1b[7$"],
        "end" => &["\x1b[8$"],
        _ => &[],
    }
}

fn legacy_ctrl_sequences(key: &str) -> &'static [&'static str] {
    match key {
        "up" => &["\x1bOa"],
        "down" => &["\x1bOb"],
        "right" => &["\x1bOc"],
        "left" => &["\x1bOd"],
        "clear" => &["\x1bOe"],
        "insert" => &["\x1b[2^"],
        "delete" => &["\x1b[3^"],
        "pageUp" => &["\x1b[5^"],
        "pageDown" => &["\x1b[6^"],
        "home" => &["\x1b[7^"],
        "end" => &["\x1b[8^"],
        _ => &[],
    }
}

fn matches_legacy_sequence(data: &str, sequences: &[&str]) -> bool {
    sequences.contains(&data)
}

fn matches_legacy_modifier_sequence(data: &str, key: &str, modifier: u32) -> bool {
    if modifier == MOD_SHIFT {
        return matches_legacy_sequence(data, legacy_shift_sequences(key));
    }
    if modifier == MOD_CTRL {
        return matches_legacy_sequence(data, legacy_ctrl_sequences(key));
    }
    false
}

// ===========================================================================
// Kitty sequence matching
// ===========================================================================

fn matches_kitty_sequence(data: &str, expected_codepoint: i32, expected_modifier: u32) -> bool {
    let Some(parsed) = parse_kitty_sequence(data) else {
        return false;
    };
    let actual_mod = parsed.modifier & !LOCK_MASK;
    let expected_mod = expected_modifier & !LOCK_MASK;

    if actual_mod != expected_mod {
        return false;
    }

    let normalized_cp = normalize_shifted_letter_identity(
        kitty_functional_equivalent(parsed.codepoint),
        parsed.modifier,
    );
    let normalized_expected = normalize_shifted_letter_identity(
        kitty_functional_equivalent(expected_codepoint),
        expected_modifier,
    );

    if normalized_cp == normalized_expected {
        return true;
    }

    // Alternate match via base layout key
    if let Some(base) = parsed.base_layout_key
        && base == expected_codepoint
    {
        let is_latin = (97..=122).contains(&normalized_cp);
        let is_known_symbol =
            char::from_u32(normalized_cp.cast_unsigned()).is_some_and(is_symbol_key);
        if !is_latin && !is_known_symbol {
            return true;
        }
    }

    false
}

fn matches_modify_other_keys(data: &str, expected_keycode: i32, expected_modifier: u32) -> bool {
    let Some(parsed) = parse_modify_other_keys_sequence(data) else {
        return false;
    };
    parsed.codepoint == expected_keycode && parsed.modifier == expected_modifier
}

fn matches_printable_modify_other_keys(
    data: &str,
    expected_keycode: i32,
    expected_modifier: u32,
) -> bool {
    if expected_modifier == 0 {
        return false;
    }
    let Some(parsed) = parse_modify_other_keys_sequence(data) else {
        return false;
    };
    if parsed.modifier != expected_modifier {
        return false;
    }
    normalize_shifted_letter_identity(parsed.codepoint, parsed.modifier)
        == normalize_shifted_letter_identity(expected_keycode, expected_modifier)
}

fn matches_raw_backspace(data: &str, expected_modifier: u32) -> bool {
    matches!(data, "\x7f" | "\x08") && expected_modifier == 0
}

// ===========================================================================
// Key release / repeat detection
// ===========================================================================

/// Check if the sequence is a key release event.
#[must_use]
pub fn is_key_release(data: &str) -> bool {
    if data.contains("\x1b[200~") {
        return false;
    }
    data.contains(":3u")
        || data.contains(":3~")
        || data.contains(":3A")
        || data.contains(":3B")
        || data.contains(":3C")
        || data.contains(":3D")
        || data.contains(":3H")
        || data.contains(":3F")
}

/// Check if the sequence is a key repeat event.
#[must_use]
pub fn is_key_repeat(data: &str) -> bool {
    if data.contains("\x1b[200~") {
        return false;
    }
    data.contains(":2u")
        || data.contains(":2~")
        || data.contains(":2A")
        || data.contains(":2B")
        || data.contains(":2C")
        || data.contains(":2D")
        || data.contains(":2H")
        || data.contains(":2F")
}

// ===========================================================================
// parse_key — port of parseKey()
// ===========================================================================

/// Parse input data and return the key identifier if recognized.
///
/// Returns a normalized key id like `"ctrl+c"`, `"shift+enter"`, `"up"`, etc.
#[must_use]
#[allow(clippy::too_many_lines)]
pub fn parse_key(data: &str) -> Option<String> {
    // Kitty CSI-u sequences
    if let Some(kitty) = parse_kitty_sequence(data) {
        return format_parsed_key(kitty.codepoint, kitty.modifier, kitty.base_layout_key);
    }

    // modifyOtherKeys sequences
    if let Some(mok) = parse_modify_other_keys_sequence(data) {
        return format_parsed_key(mok.codepoint, mok.modifier, None);
    }

    // Mode-aware legacy sequences
    if is_kitty_protocol_active() && (data == "\x1b\r" || data == "\n") {
        return Some("shift+enter".to_owned());
    }

    // Legacy sequence lookup table
    if let Some(id) = legacy_sequence_key_id(data) {
        return Some(id.to_owned());
    }

    // Individual legacy sequences
    if data == "\x1b" {
        return Some("escape".to_owned());
    }
    if data == "\x1c" {
        return Some("ctrl+\\".to_owned());
    }
    if data == "\x1d" {
        return Some("ctrl+]".to_owned());
    }
    if data == "\x1f" {
        return Some("ctrl+-".to_owned());
    }
    if data == "\x1b\x1b" {
        return Some("ctrl+alt+[".to_owned());
    }
    if data == "\x1b\x1c" {
        return Some("ctrl+alt+\\".to_owned());
    }
    if data == "\x1b\x1d" {
        return Some("ctrl+alt+]".to_owned());
    }
    if data == "\x1b\x1f" {
        return Some("ctrl+alt+-".to_owned());
    }
    if data == "\t" {
        return Some("tab".to_owned());
    }
    if data == "\r" || (!is_kitty_protocol_active() && data == "\n") || data == "\x1bOM" {
        return Some("enter".to_owned());
    }
    if data == "\x00" {
        return Some("ctrl+space".to_owned());
    }
    if data == " " {
        return Some("space".to_owned());
    }
    if data == "\x7f" {
        return Some("backspace".to_owned());
    }
    if data == "\x08" {
        return Some("backspace".to_owned());
    }
    if data == "\x1b[Z" {
        return Some("shift+tab".to_owned());
    }
    if !is_kitty_protocol_active() && data == "\x1b\r" {
        return Some("alt+enter".to_owned());
    }
    if !is_kitty_protocol_active() && data == "\x1b " {
        return Some("alt+space".to_owned());
    }
    if data == "\x1b\x7f" || data == "\x1b\x08" {
        return Some("alt+backspace".to_owned());
    }
    if !is_kitty_protocol_active() && data == "\x1bB" {
        return Some("alt+left".to_owned());
    }
    if !is_kitty_protocol_active() && data == "\x1bF" {
        return Some("alt+right".to_owned());
    }

    // ESC + single char (2 bytes)
    if !is_kitty_protocol_active() && data.len() == 2 && data.starts_with('\x1b') {
        let code = data.as_bytes()[1];
        if (1..=26).contains(&code) {
            return Some(format!("ctrl+alt+{}", char::from(code + 96)));
        }
        if code.is_ascii_alphabetic() || (48..=57).contains(&code) {
            return Some(format!("alt+{}", char::from(code.to_ascii_lowercase())));
        }
    }

    // Additional legacy sequences
    if data == "\x1b[A" {
        return Some("up".to_owned());
    }
    if data == "\x1b[B" {
        return Some("down".to_owned());
    }
    if data == "\x1b[C" {
        return Some("right".to_owned());
    }
    if data == "\x1b[D" {
        return Some("left".to_owned());
    }
    if data == "\x1b[H" || data == "\x1bOH" {
        return Some("home".to_owned());
    }
    if data == "\x1b[F" || data == "\x1bOF" {
        return Some("end".to_owned());
    }
    if data == "\x1b[3~" {
        return Some("delete".to_owned());
    }
    if data == "\x1b[5~" {
        return Some("pageUp".to_owned());
    }
    if data == "\x1b[6~" {
        return Some("pageDown".to_owned());
    }

    // Raw Ctrl+letter
    if data.len() == 1 {
        let code = data.as_bytes()[0];
        if (1..=26).contains(&code) {
            return Some(format!("ctrl+{}", char::from(code + 96)));
        }
        if (32..=126).contains(&code) {
            return Some(data.to_owned());
        }
    }

    // Multi-byte UTF-8 printable characters (e.g. CJK, emoji, full-width symbols).
    // These arrive as raw bytes from the terminal's text input path.
    // Only accept if the entire string is a single character (no modifiers).
    if data.chars().count() == 1 {
        let ch = data.chars().next()?;
        if (ch as u32) >= 0x80 && !ch.is_control() {
            return Some(data.to_owned());
        }
    }

    None
}

// ===========================================================================
// matches_key — port of matchesKey()
// ===========================================================================

/// Parse a key id string into its components.
fn parse_key_id(key_id: &str) -> Option<(String, bool, bool, bool, bool)> {
    let lower = key_id.to_lowercase();
    let parts: Vec<&str> = lower.split('+').collect();
    let key = parts.last()?.to_string();
    Some((
        key,
        parts.contains(&"ctrl"),
        parts.contains(&"shift"),
        parts.contains(&"alt"),
        parts.contains(&"super"),
    ))
}

/// Check if input data matches a key identifier string.
#[must_use]
#[allow(clippy::too_many_lines)]
pub fn matches_key(data: &str, key_id: &str) -> bool {
    let Some((key, ctrl, shift, alt, super_mod)) = parse_key_id(key_id) else {
        return false;
    };

    let mut modifier: u32 = 0;
    if shift {
        modifier |= MOD_SHIFT;
    }
    if alt {
        modifier |= MOD_ALT;
    }
    if ctrl {
        modifier |= MOD_CTRL;
    }
    if super_mod {
        modifier |= MOD_SUPER;
    }

    match key.as_str() {
        "escape" | "esc" => {
            if modifier != 0 {
                return false;
            }
            data == "\x1b"
                || matches_kitty_sequence(data, CP_ESCAPE, 0)
                || matches_modify_other_keys(data, CP_ESCAPE, 0)
        }

        "space" => {
            if !is_kitty_protocol_active() {
                if modifier == MOD_CTRL && data == "\x00" {
                    return true;
                }
                if modifier == MOD_ALT && data == "\x1b " {
                    return true;
                }
            }
            if modifier == 0 {
                return data == " "
                    || matches_kitty_sequence(data, CP_SPACE, 0)
                    || matches_modify_other_keys(data, CP_SPACE, 0);
            }
            matches_kitty_sequence(data, CP_SPACE, modifier)
                || matches_modify_other_keys(data, CP_SPACE, modifier)
        }

        "tab" => {
            if modifier == MOD_SHIFT {
                return data == "\x1b[Z"
                    || matches_kitty_sequence(data, CP_TAB, MOD_SHIFT)
                    || matches_modify_other_keys(data, CP_TAB, MOD_SHIFT);
            }
            if modifier == 0 {
                return data == "\t" || matches_kitty_sequence(data, CP_TAB, 0);
            }
            matches_kitty_sequence(data, CP_TAB, modifier)
                || matches_modify_other_keys(data, CP_TAB, modifier)
        }

        "enter" | "return" => match modifier {
            MOD_SHIFT => {
                if matches_kitty_sequence(data, CP_ENTER, MOD_SHIFT)
                    || matches_kitty_sequence(data, CP_KP_ENTER, MOD_SHIFT)
                    || matches_modify_other_keys(data, CP_ENTER, MOD_SHIFT)
                {
                    return true;
                }
                if is_kitty_protocol_active() {
                    return data == "\x1b\r" || data == "\n";
                }
                false
            }
            MOD_ALT => {
                if matches_kitty_sequence(data, CP_ENTER, MOD_ALT)
                    || matches_kitty_sequence(data, CP_KP_ENTER, MOD_ALT)
                    || matches_modify_other_keys(data, CP_ENTER, MOD_ALT)
                {
                    return true;
                }
                if !is_kitty_protocol_active() {
                    return data == "\x1b\r";
                }
                false
            }
            0 => {
                data == "\r"
                    || (!is_kitty_protocol_active() && data == "\n")
                    || data == "\x1bOM"
                    || matches_kitty_sequence(data, CP_ENTER, 0)
                    || matches_kitty_sequence(data, CP_KP_ENTER, 0)
            }
            _ => {
                matches_kitty_sequence(data, CP_ENTER, modifier)
                    || matches_kitty_sequence(data, CP_KP_ENTER, modifier)
                    || matches_modify_other_keys(data, CP_ENTER, modifier)
            }
        },

        "backspace" => {
            if modifier == MOD_ALT {
                if data == "\x1b\x7f" || data == "\x1b\x08" {
                    return true;
                }
                return matches_kitty_sequence(data, CP_BACKSPACE, MOD_ALT)
                    || matches_modify_other_keys(data, CP_BACKSPACE, MOD_ALT);
            }
            if modifier == MOD_CTRL {
                if matches_raw_backspace(data, MOD_CTRL) {
                    return true;
                }
                return matches_kitty_sequence(data, CP_BACKSPACE, MOD_CTRL)
                    || matches_modify_other_keys(data, CP_BACKSPACE, MOD_CTRL);
            }
            if modifier == 0 {
                return matches_raw_backspace(data, 0)
                    || matches_kitty_sequence(data, CP_BACKSPACE, 0)
                    || matches_modify_other_keys(data, CP_BACKSPACE, 0);
            }
            matches_kitty_sequence(data, CP_BACKSPACE, modifier)
                || matches_modify_other_keys(data, CP_BACKSPACE, modifier)
        }

        "insert" => {
            if modifier == 0 {
                return matches_legacy_sequence(data, legacy_key_sequences("insert"))
                    || matches_kitty_sequence(data, CP_INSERT, 0);
            }
            matches_legacy_modifier_sequence(data, "insert", modifier)
                || matches_kitty_sequence(data, CP_INSERT, modifier)
        }

        "delete" => {
            if modifier == 0 {
                return matches_legacy_sequence(data, legacy_key_sequences("delete"))
                    || matches_kitty_sequence(data, CP_DELETE, 0);
            }
            matches_legacy_modifier_sequence(data, "delete", modifier)
                || matches_kitty_sequence(data, CP_DELETE, modifier)
        }

        "clear" => {
            if modifier == 0 {
                return matches_legacy_sequence(data, legacy_key_sequences("clear"));
            }
            matches_legacy_modifier_sequence(data, "clear", modifier)
        }

        "home" => {
            if modifier == 0 {
                return matches_legacy_sequence(data, legacy_key_sequences("home"))
                    || matches_kitty_sequence(data, CP_HOME, 0);
            }
            matches_legacy_modifier_sequence(data, "home", modifier)
                || matches_kitty_sequence(data, CP_HOME, modifier)
        }

        "end" => {
            if modifier == 0 {
                return matches_legacy_sequence(data, legacy_key_sequences("end"))
                    || matches_kitty_sequence(data, CP_END, 0);
            }
            matches_legacy_modifier_sequence(data, "end", modifier)
                || matches_kitty_sequence(data, CP_END, modifier)
        }

        "pageup" => {
            if modifier == 0 {
                return matches_legacy_sequence(data, legacy_key_sequences("pageUp"))
                    || matches_kitty_sequence(data, CP_PAGE_UP, 0);
            }
            matches_legacy_modifier_sequence(data, "pageUp", modifier)
                || matches_kitty_sequence(data, CP_PAGE_UP, modifier)
        }

        "pagedown" => {
            if modifier == 0 {
                return matches_legacy_sequence(data, legacy_key_sequences("pageDown"))
                    || matches_kitty_sequence(data, CP_PAGE_DOWN, 0);
            }
            matches_legacy_modifier_sequence(data, "pageDown", modifier)
                || matches_kitty_sequence(data, CP_PAGE_DOWN, modifier)
        }

        "up" => {
            if modifier == MOD_ALT {
                return data == "\x1bp"
                    || data == "\x1b\x1b[A"
                    || data == "\x1b\x1bOA"
                    || matches_kitty_sequence(data, CP_UP, MOD_ALT);
            }
            if modifier == 0 {
                return matches_legacy_sequence(data, legacy_key_sequences("up"))
                    || matches_kitty_sequence(data, CP_UP, 0);
            }
            matches_legacy_modifier_sequence(data, "up", modifier)
                || matches_kitty_sequence(data, CP_UP, modifier)
        }

        "down" => {
            if modifier == MOD_ALT {
                return data == "\x1bn"
                    || data == "\x1b\x1b[B"
                    || data == "\x1b\x1bOB"
                    || matches_kitty_sequence(data, CP_DOWN, MOD_ALT);
            }
            if modifier == 0 {
                return matches_legacy_sequence(data, legacy_key_sequences("down"))
                    || matches_kitty_sequence(data, CP_DOWN, 0);
            }
            matches_legacy_modifier_sequence(data, "down", modifier)
                || matches_kitty_sequence(data, CP_DOWN, modifier)
        }

        "left" => {
            if modifier == MOD_ALT {
                return data == "\x1b[1;3D"
                    || (!is_kitty_protocol_active() && data == "\x1bB")
                    || data == "\x1bb"
                    || data == "\x1b\x1b[D"
                    || data == "\x1b\x1bOD"
                    || matches_kitty_sequence(data, CP_LEFT, MOD_ALT);
            }
            if modifier == MOD_CTRL {
                return data == "\x1b[1;5D"
                    || matches_legacy_modifier_sequence(data, "left", MOD_CTRL)
                    || matches_kitty_sequence(data, CP_LEFT, MOD_CTRL);
            }
            if modifier == 0 {
                return matches_legacy_sequence(data, legacy_key_sequences("left"))
                    || matches_kitty_sequence(data, CP_LEFT, 0);
            }
            matches_legacy_modifier_sequence(data, "left", modifier)
                || matches_kitty_sequence(data, CP_LEFT, modifier)
        }

        "right" => {
            if modifier == MOD_ALT {
                return data == "\x1b[1;3C"
                    || (!is_kitty_protocol_active() && data == "\x1bF")
                    || data == "\x1bf"
                    || data == "\x1b\x1b[C"
                    || data == "\x1b\x1bOC"
                    || matches_kitty_sequence(data, CP_RIGHT, MOD_ALT);
            }
            if modifier == MOD_CTRL {
                return data == "\x1b[1;5C"
                    || matches_legacy_modifier_sequence(data, "right", MOD_CTRL)
                    || matches_kitty_sequence(data, CP_RIGHT, MOD_CTRL);
            }
            if modifier == 0 {
                return matches_legacy_sequence(data, legacy_key_sequences("right"))
                    || matches_kitty_sequence(data, CP_RIGHT, 0);
            }
            matches_legacy_modifier_sequence(data, "right", modifier)
                || matches_kitty_sequence(data, CP_RIGHT, modifier)
        }

        "f1" | "f2" | "f3" | "f4" | "f5" | "f6" | "f7" | "f8" | "f9" | "f10" | "f11" | "f12" => {
            if modifier != 0 {
                return false;
            }
            matches_legacy_sequence(data, legacy_key_sequences(&key))
        }

        _ => {
            // Handle single letter/digit keys and symbols
            let Some(c) = key.chars().next() else {
                return false;
            };
            let codepoint = c as i32;
            let is_letter = c.is_ascii_lowercase();
            let is_digit = c.is_ascii_digit();

            if !(is_letter || is_digit || is_symbol_key(c)) {
                return false;
            }

            let raw_ctrl = raw_ctrl_char(&key);

            // Legacy: ctrl+alt+key = ESC + control character
            if modifier == MOD_CTRL + MOD_ALT
                && !is_kitty_protocol_active()
                && let Some(rc) = raw_ctrl
            {
                let expected = format!("\x1b{rc}");
                if data == expected {
                    return true;
                }
            }

            // Legacy: alt+letter/digit = ESC + key
            if modifier == MOD_ALT && !is_kitty_protocol_active() && (is_letter || is_digit) {
                let expected = format!("\x1b{c}");
                if data == expected {
                    return true;
                }
            }

            if modifier == MOD_CTRL {
                if let Some(rc) = raw_ctrl
                    && data == rc.to_string()
                {
                    return true;
                }
                return matches_kitty_sequence(data, codepoint, MOD_CTRL)
                    || matches_printable_modify_other_keys(data, codepoint, MOD_CTRL);
            }

            if modifier == MOD_SHIFT + MOD_CTRL {
                return matches_kitty_sequence(data, codepoint, MOD_SHIFT + MOD_CTRL)
                    || matches_printable_modify_other_keys(data, codepoint, MOD_SHIFT + MOD_CTRL);
            }

            if modifier == MOD_SHIFT {
                if is_letter {
                    let upper = c.to_ascii_uppercase();
                    if data == upper.to_string() {
                        return true;
                    }
                }
                return matches_kitty_sequence(data, codepoint, MOD_SHIFT)
                    || matches_printable_modify_other_keys(data, codepoint, MOD_SHIFT);
            }

            if modifier != 0 {
                return matches_kitty_sequence(data, codepoint, modifier)
                    || matches_printable_modify_other_keys(data, codepoint, modifier);
            }

            data == key || matches_kitty_sequence(data, codepoint, 0)
        }
    }
}

// ===========================================================================
// Printable key decoding (from decodePrintableKey)
// ===========================================================================

/// Decode a Kitty CSI-u or modifyOtherKeys sequence into a printable character.
///
/// Only accepts plain or Shift-modified keys. Rejects Ctrl, Alt, and
/// unsupported modifier combinations.
#[must_use]
pub fn decode_printable_key(data: &str) -> Option<char> {
    decode_kitty_printable(data).or_else(|| decode_modify_other_keys_printable(data))
}

fn decode_kitty_printable(data: &str) -> Option<char> {
    let caps = csi_u_regex().captures(data)?;
    let codepoint: i32 = caps.get(1)?.as_str().parse().ok()?;

    let shifted_key = caps
        .get(2)
        .filter(|m| !m.as_str().is_empty())
        .and_then(|m| m.as_str().parse::<i32>().ok());

    let mod_value: u32 = caps
        .get(4)
        .and_then(|m| m.as_str().parse().ok())
        .unwrap_or(1);
    let modifier = mod_value.saturating_sub(1);

    let allowed = MOD_SHIFT | LOCK_MASK;
    if (modifier & !allowed) != 0 {
        return None;
    }
    if (modifier & (MOD_ALT | MOD_CTRL)) != 0 {
        return None;
    }

    let mut effective = codepoint;
    if (modifier & MOD_SHIFT) != 0
        && let Some(sk) = shifted_key
    {
        effective = sk;
    }
    effective = kitty_functional_equivalent(effective);
    if effective < 32 {
        return None;
    }
    char::from_u32(effective.cast_unsigned())
}

fn decode_modify_other_keys_printable(data: &str) -> Option<char> {
    let parsed = parse_modify_other_keys_sequence(data)?;
    let modifier = parsed.modifier & !LOCK_MASK;
    if (modifier & !MOD_SHIFT) != 0 {
        return None;
    }
    if parsed.codepoint < 32 {
        return None;
    }
    char::from_u32(parsed.codepoint.cast_unsigned())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_ctrl_c_legacy() {
        assert_eq!(parse_key("\x03"), Some("ctrl+c".to_owned()));
    }

    #[test]
    fn parse_ctrl_v_legacy() {
        assert_eq!(parse_key("\x16"), Some("ctrl+v".to_owned()));
    }

    #[test]
    fn parse_ctrl_v_kitty() {
        // CSI-u format for Ctrl+V: codepoint 118, modifier 5 (ctrl=4, 1-indexed=5)
        assert_eq!(parse_key("\x1b[118;5u"), Some("ctrl+v".to_owned()));
    }

    #[test]
    fn matches_ctrl_v_legacy() {
        assert!(matches_key("\x16", "ctrl+v"));
    }

    #[test]
    fn matches_ctrl_v_kitty() {
        assert!(matches_key("\x1b[118;5u", "ctrl+v"));
    }

    #[test]
    fn parse_enter() {
        assert_eq!(parse_key("\r"), Some("enter".to_owned()));
    }

    #[test]
    fn parse_escape() {
        assert_eq!(parse_key("\x1b"), Some("escape".to_owned()));
    }

    #[test]
    fn parse_shift_tab() {
        assert_eq!(parse_key("\x1b[Z"), Some("shift+tab".to_owned()));
    }

    #[test]
    fn parse_backspace() {
        assert_eq!(parse_key("\x7f"), Some("backspace".to_owned()));
    }

    #[test]
    fn bracketed_paste_single_chunk() {
        let mut parser = RawInputParser::new();
        let events = parser.feed_bytes(b"\x1b[200~hello\x1b[201~");
        assert_eq!(events, vec![RawEvent::Paste("hello".to_owned())]);
    }

    #[test]
    fn bracketed_paste_multi_chunk() {
        let mut parser = RawInputParser::new();
        let events = parser.feed_bytes(b"\x1b[200~hel");
        assert!(events.is_empty());
        let events = parser.feed_bytes(b"lo\x1b[201~");
        assert_eq!(events, vec![RawEvent::Paste("hello".to_owned())]);
    }

    #[test]
    fn key_after_paste() {
        let mut parser = RawInputParser::new();
        parser.feed_bytes(b"\x1b[200~paste\x1b[201~");
        let events = parser.feed_bytes(b"x");
        assert_eq!(events, vec![RawEvent::Key("x".to_owned())]);
    }

    #[test]
    fn ctrl_c_then_ctrl_v_produces_two_events() {
        let mut parser = RawInputParser::new();
        let events = parser.feed_bytes(b"\x03\x16");
        assert_eq!(
            events,
            vec![
                RawEvent::Key("\x03".to_owned()),
                RawEvent::Key("\x16".to_owned()),
            ]
        );
    }

    #[test]
    fn arrow_up_sequence() {
        let mut parser = RawInputParser::new();
        let events = parser.feed_bytes(b"\x1b[A");
        assert_eq!(events, vec![RawEvent::Key("\x1b[A".to_owned())]);
        assert_eq!(parse_key("\x1b[A"), Some("up".to_owned()));
    }

    #[test]
    fn meta_arrow_up_sequence() {
        let mut parser = RawInputParser::new();
        let events = parser.feed_bytes(b"\x1b\x1b[A");
        assert_eq!(events, vec![RawEvent::Key("\x1b\x1b[A".to_owned())]);
        assert_eq!(parse_key("\x1b\x1b[A"), Some("alt+up".to_owned()));
        assert!(matches_key("\x1b\x1b[A", "alt+up"));
    }

    #[test]
    fn flush_lone_esc() {
        let mut parser = RawInputParser::new();
        let events = parser.feed_bytes(b"\x1b");
        assert!(events.is_empty());
        let events = parser.flush();
        assert_eq!(events, vec![RawEvent::Key("\x1b".to_owned())]);
    }

    #[test]
    fn esc_enter_single_sequence() {
        let mut parser = RawInputParser::new();
        let events = parser.feed_bytes(b"\x1b\r");
        assert_eq!(events, vec![RawEvent::Key("\x1b\r".to_owned())]);
    }

    #[test]
    fn decode_printable_kitty_a() {
        // CSI-u for plain 'a': codepoint 97, no modifiers
        assert_eq!(decode_printable_key("\x1b[97u"), Some('a'));
    }

    #[test]
    fn decode_printable_kitty_shift_a() {
        // CSI-u for Shift+a (A): codepoint 97, shifted 65, modifier 2 (shift)
        assert_eq!(decode_printable_key("\x1b[97:65;2u"), Some('A'));
    }

    #[test]
    fn decode_printable_rejects_ctrl() {
        // Ctrl+v should not be decoded as printable
        assert_eq!(decode_printable_key("\x1b[118;5u"), None);
    }

    #[test]
    fn is_key_release_detection() {
        assert!(is_key_release("\x1b[97;5:3u"));
        assert!(!is_key_release("\x1b[97;5u"));
        assert!(!is_key_release("\x1b[200~some paste:3u"));
    }

    #[test]
    fn parse_cjk_character() {
        // CJK character 你 (U+4F60, UTF-8: E4 BD A0)
        assert_eq!(parse_key("你"), Some("你".to_owned()));
    }

    #[test]
    fn parse_emoji_character() {
        // Emoji 😀 (U+1F600)
        assert_eq!(parse_key("😀"), Some("😀".to_owned()));
    }

    #[test]
    fn parse_fullwidth_symbol() {
        // Full-width comma （U+FF0C, UTF-8: EF BC 8C）
        assert_eq!(parse_key("，"), Some("，".to_owned()));
    }

    #[test]
    fn feed_bytes_cjk_character() {
        let mut parser = RawInputParser::new();
        let events = parser.feed_bytes("你".as_bytes());
        assert_eq!(events.len(), 1);
        assert_eq!(events[0], RawEvent::Key("你".to_owned()));
    }

    #[test]
    fn feed_bytes_multiple_cjk() {
        let mut parser = RawInputParser::new();
        let events = parser.feed_bytes("你好".as_bytes());
        assert_eq!(events.len(), 2);
        assert_eq!(events[0], RawEvent::Key("你".to_owned()));
        assert_eq!(events[1], RawEvent::Key("好".to_owned()));
    }

    #[test]
    fn feed_bytes_space() {
        let mut parser = RawInputParser::new();
        let events = parser.feed_bytes(b" ");
        assert_eq!(events.len(), 1);
        assert_eq!(events[0], RawEvent::Key(" ".to_owned()));
    }
}
