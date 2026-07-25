use std::io::{Write, stdout};

use crossterm::event::{
    DisableBracketedPaste, DisableMouseCapture, EnableBracketedPaste, EnableMouseCapture,
    KeyboardEnhancementFlags, PopKeyboardEnhancementFlags, PushKeyboardEnhancementFlags,
};
use crossterm::terminal::{
    EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode,
};
use crossterm::{execute, queue};

use crate::terminal_capabilities::TerminalCapabilities;

pub(super) fn write_enter_output(
    output: &mut dyn Write,
    capabilities: TerminalCapabilities,
) -> std::io::Result<()> {
    let mut output = output;
    if capabilities.ansi.bracketed_paste {
        queue!(&mut output, EnableBracketedPaste)?;
    }
    if capabilities.ansi.kitty_keyboard {
        queue!(
            &mut output,
            PushKeyboardEnhancementFlags(
                KeyboardEnhancementFlags::DISAMBIGUATE_ESCAPE_CODES
                    | KeyboardEnhancementFlags::REPORT_EVENT_TYPES
                    | KeyboardEnhancementFlags::REPORT_ALTERNATE_KEYS,
            )
        )?;
    }
    output.flush()
}

pub(super) fn write_leave_output(
    output: &mut dyn Write,
    capabilities: TerminalCapabilities,
) -> std::io::Result<()> {
    let mut output = output;
    let mut result = output.write_all(b"\x1b[?25h");
    if capabilities.ansi.kitty_keyboard
        && let Err(error) = execute!(&mut output, PopKeyboardEnhancementFlags)
    {
        result = Err(error);
    }
    if capabilities.ansi.bracketed_paste
        && let Err(error) = execute!(&mut output, DisableBracketedPaste)
        && result.is_ok()
    {
        result = Err(error);
    }
    result
}

pub(super) fn write_enter_review_output(output: &mut dyn Write) -> std::io::Result<()> {
    let mut output = output;
    queue!(&mut output, EnterAlternateScreen)?;
    output.flush()
}

pub(super) fn write_leave_review_output(output: &mut dyn Write) -> std::io::Result<()> {
    let mut output = output;
    queue!(&mut output, LeaveAlternateScreen)?;
    output.flush()
}

pub(super) fn write_enable_mouse_capture(output: &mut dyn Write) -> std::io::Result<()> {
    let mut output = output;
    queue!(&mut output, EnableMouseCapture)
}

pub(super) fn write_disable_mouse_capture(output: &mut dyn Write) -> std::io::Result<()> {
    let mut output = output;
    queue!(&mut output, DisableMouseCapture)
}

#[derive(Debug)]
pub(super) struct TerminalModeGuard {
    capabilities: TerminalCapabilities,
    active: bool,
    review_active: bool,
    mouse_capture_active: bool,
    #[cfg(windows)]
    windows_input_mode: windows_input_mode::WindowsInputModeGuard,
}

impl TerminalModeGuard {
    pub(super) fn enter(capabilities: TerminalCapabilities) -> std::io::Result<Self> {
        let raw_mode = RawModeGuard::enter()?;
        #[cfg(windows)]
        let mut windows_input_mode = windows_input_mode::WindowsInputModeGuard::enter()?;
        let mut output = stdout();
        if let Err(error) = write_enter_output(&mut output, capabilities) {
            let _ = write_leave_output(&mut output, capabilities);
            #[cfg(windows)]
            windows_input_mode.restore();
            return Err(error);
        }
        raw_mode.disarm();
        Ok(Self {
            capabilities,
            active: true,
            review_active: false,
            mouse_capture_active: false,
            #[cfg(windows)]
            windows_input_mode,
        })
    }

    pub(super) fn leave(&mut self) {
        if !self.active {
            if self.mouse_capture_active || self.review_active {
                let mut output = stdout();
                if self.mouse_capture_active {
                    let _ = write_disable_mouse_capture(&mut output);
                    self.mouse_capture_active = false;
                }
                if self.review_active {
                    let _ = write_leave_review_output(&mut output);
                    self.review_active = false;
                }
                let _ = output.flush();
            }
            return;
        }
        let mut output = stdout();
        if self.mouse_capture_active {
            let _ = write_disable_mouse_capture(&mut output);
            self.mouse_capture_active = false;
        }
        if self.review_active {
            let _ = write_leave_review_output(&mut output);
            self.review_active = false;
        }
        let _ = write_leave_output(&mut output, self.capabilities);
        let _ = output.flush();
        #[cfg(windows)]
        self.windows_input_mode.restore();
        let _ = disable_raw_mode();
        self.active = false;
    }

    pub(super) fn resume(&mut self) -> std::io::Result<()> {
        if self.active {
            return Ok(());
        }
        self.review_active = false;
        self.mouse_capture_active = false;
        let raw_mode = RawModeGuard::enter()?;
        #[cfg(windows)]
        {
            self.windows_input_mode = windows_input_mode::WindowsInputModeGuard::enter()?;
        }
        let mut output = stdout();
        if let Err(error) = write_enter_output(&mut output, self.capabilities) {
            let _ = write_leave_output(&mut output, self.capabilities);
            #[cfg(windows)]
            self.windows_input_mode.restore();
            return Err(error);
        }
        raw_mode.disarm();
        self.active = true;
        Ok(())
    }

    pub(super) fn enter_review(&mut self, output: &mut dyn Write) -> std::io::Result<()> {
        if !self.active || self.review_active {
            return Ok(());
        }
        write_enter_review_output(output)
    }

    pub(super) fn leave_review(&mut self, output: &mut dyn Write) -> std::io::Result<()> {
        if !self.review_active {
            return Ok(());
        }
        write_leave_review_output(output)
    }

    pub(super) const fn set_review_active(&mut self, active: bool) {
        self.review_active = active;
    }

    pub(super) const fn set_mouse_capture_active(&mut self, active: bool) {
        self.mouse_capture_active = active;
    }

    #[cfg(test)]
    pub(super) fn for_test() -> Self {
        Self {
            capabilities: TerminalCapabilities::default(),
            active: true,
            review_active: false,
            mouse_capture_active: false,
            #[cfg(windows)]
            windows_input_mode: windows_input_mode::WindowsInputModeGuard::for_test(),
        }
    }

    #[cfg(test)]
    pub(super) const fn review_active_for_test(&self) -> bool {
        self.review_active
    }

    #[cfg(test)]
    pub(super) const fn active_for_test(&self) -> bool {
        self.active
    }

    #[cfg(test)]
    pub(super) const fn disarm_for_test(&mut self) {
        self.active = false;
        self.review_active = false;
        self.mouse_capture_active = false;
    }
}

impl Drop for TerminalModeGuard {
    fn drop(&mut self) {
        self.leave();
    }
}

struct RawModeGuard {
    active: bool,
}

impl RawModeGuard {
    fn enter() -> std::io::Result<Self> {
        enable_raw_mode()?;
        Ok(Self { active: true })
    }

    fn disarm(mut self) {
        self.active = false;
    }
}

impl Drop for RawModeGuard {
    fn drop(&mut self) {
        if self.active {
            let _ = disable_raw_mode();
        }
    }
}

#[cfg(any(windows, test))]
mod windows_input_mode {
    use std::io;

    const ENABLE_VIRTUAL_TERMINAL_INPUT: u32 = 0x0200;

    /// Private console mode query/set seam so tests can inject deterministic failures
    /// without a public trait or second production path.
    #[derive(Clone, Copy)]
    struct ConsoleModeOps {
        query: fn() -> io::Result<u32>,
        set: fn(u32) -> io::Result<()>,
    }

    #[cfg(windows)]
    fn query_console_mode() -> io::Result<u32> {
        winapi_util::console::mode(&io::stdin())
    }

    #[cfg(windows)]
    fn set_console_mode(mode: u32) -> io::Result<()> {
        winapi_util::console::set_mode(&io::stdin(), mode)
    }

    // Non-Windows stubs exist only so `cfg(test)` can compile the seamed unit
    // tests on Unix hosts. Production entry uses this module solely on Windows.
    #[cfg(not(windows))]
    fn query_console_mode() -> io::Result<u32> {
        Err(io::Error::new(
            io::ErrorKind::Unsupported,
            "windows console mode is unavailable on this host",
        ))
    }

    #[cfg(not(windows))]
    fn set_console_mode(_mode: u32) -> io::Result<()> {
        Err(io::Error::new(
            io::ErrorKind::Unsupported,
            "windows console mode is unavailable on this host",
        ))
    }

    const fn default_console_mode_ops() -> ConsoleModeOps {
        ConsoleModeOps {
            query: query_console_mode,
            set: set_console_mode,
        }
    }

    #[derive(Debug, Clone, Copy)]
    pub(super) struct WindowsInputModeGuard {
        original_mode: u32,
        changed: bool,
    }

    impl WindowsInputModeGuard {
        const fn inactive() -> Self {
            Self {
                original_mode: 0,
                changed: false,
            }
        }

        #[cfg(test)]
        pub(super) const fn for_test() -> Self {
            Self::inactive()
        }

        pub(super) fn enter() -> io::Result<Self> {
            Self::enter_with(default_console_mode_ops())
        }

        fn enter_with(ops: ConsoleModeOps) -> io::Result<Self> {
            // Query failure must abort entry — never continue with unknown input semantics.
            let mode = (ops.query)()?;
            let vt_mode = mode | ENABLE_VIRTUAL_TERMINAL_INPUT;
            if vt_mode == mode {
                return Ok(Self::inactive());
            }
            (ops.set)(vt_mode)?;
            Ok(Self {
                original_mode: mode,
                changed: true,
            })
        }

        pub(super) fn restore(&mut self) {
            self.restore_with(set_console_mode);
        }

        fn restore_with(&mut self, set: fn(u32) -> io::Result<()>) {
            if !self.changed {
                return;
            }
            let _ = set(self.original_mode);
            self.changed = false;
        }
    }

    #[cfg(test)]
    mod tests {
        use super::*;
        use std::sync::atomic::{AtomicU32, Ordering};

        #[test]
        fn query_failure_aborts_entry() {
            fn fail_query() -> io::Result<u32> {
                Err(io::Error::new(
                    io::ErrorKind::PermissionDenied,
                    "console mode query failed",
                ))
            }
            fn set_must_not_run(_: u32) -> io::Result<()> {
                panic!("set_mode must not run after query failure");
            }

            let error = WindowsInputModeGuard::enter_with(ConsoleModeOps {
                query: fail_query,
                set: set_must_not_run,
            })
            .expect_err("query failure must abort TUI entry");
            assert_eq!(error.kind(), io::ErrorKind::PermissionDenied);
            assert!(error.to_string().contains("console mode query failed"));
        }

        #[test]
        fn enable_and_restore_round_trip() {
            // Simulated console mode without ENABLE_VIRTUAL_TERMINAL_INPUT.
            static MODE: AtomicU32 = AtomicU32::new(0x0007);

            fn query() -> io::Result<u32> {
                Ok(MODE.load(Ordering::SeqCst))
            }
            fn set(mode: u32) -> io::Result<()> {
                MODE.store(mode, Ordering::SeqCst);
                Ok(())
            }

            let original = MODE.load(Ordering::SeqCst);
            assert_eq!(original & ENABLE_VIRTUAL_TERMINAL_INPUT, 0);

            let mut guard = WindowsInputModeGuard::enter_with(ConsoleModeOps { query, set })
                .expect("enable VT input mode");
            assert!(
                guard.changed,
                "guard must record that the console mode was changed"
            );
            assert_eq!(
                MODE.load(Ordering::SeqCst),
                original | ENABLE_VIRTUAL_TERMINAL_INPUT
            );
            assert_eq!(guard.original_mode, original);

            guard.restore_with(set);
            assert!(!guard.changed);
            assert_eq!(
                MODE.load(Ordering::SeqCst),
                original,
                "restore must write the original mode back"
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::terminal_capabilities::{AnsiCapabilities, TerminalCapabilities};

    use super::{
        write_enter_output, write_enter_review_output, write_leave_output,
        write_leave_review_output,
    };

    #[test]
    fn normal_screen_modes_never_enable_mouse_capture_or_alternate_screen() {
        let capabilities = TerminalCapabilities {
            ansi: AnsiCapabilities {
                bracketed_paste: true,
                kitty_keyboard: true,
                ..AnsiCapabilities::default()
            },
            ..TerminalCapabilities::default()
        };
        let mut enter = Vec::new();
        write_enter_output(&mut enter, capabilities).expect("enter output");
        let mut leave = Vec::new();
        write_leave_output(&mut leave, capabilities).expect("leave output");
        let output = format!(
            "{}{}",
            String::from_utf8_lossy(&enter),
            String::from_utf8_lossy(&leave)
        );

        for forbidden in [
            "\x1b[?1000h",
            "\x1b[?1002h",
            "\x1b[?1003h",
            "\x1b[?1006h",
            "\x1b[?1049h",
        ] {
            assert!(!output.contains(forbidden), "forbidden mode: {forbidden:?}");
        }
        assert!(String::from_utf8_lossy(&enter).contains("\x1b[?2004h"));
        assert!(String::from_utf8_lossy(&leave).contains("\x1b[?2004l"));
        assert!(String::from_utf8_lossy(&leave).contains("\x1b[?25h"));
        assert!(!output.contains("\x1b[2J"));
        assert!(!output.contains("\x1b[3J"));
    }

    #[test]
    fn review_modes_preserve_terminal_mouse_selection() {
        let mut enter = Vec::new();
        write_enter_review_output(&mut enter).expect("review enter output");
        let mut leave = Vec::new();
        write_leave_review_output(&mut leave).expect("review leave output");

        let enter = String::from_utf8(enter).expect("review enter is UTF-8");
        let leave = String::from_utf8(leave).expect("review leave is UTF-8");
        assert!(enter.contains("?1049h"));
        assert!(leave.contains("?1049l"));
        assert!(!format!("{enter}{leave}").contains("\x1b[2J"));
        assert!(!format!("{enter}{leave}").contains("\x1b[3J"));
        for mouse_mode in ["?1000", "?1002", "?1003", "?1006"] {
            assert!(!enter.contains(mouse_mode));
            assert!(!leave.contains(mouse_mode));
        }
    }
}
