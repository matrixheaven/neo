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

/// Enter the single fullscreen surface: alternate screen, mouse reporting,
/// bracketed paste, and (when supported) kitty keyboard enhancement.
pub(super) fn write_enter_output(
    output: &mut dyn Write,
    capabilities: TerminalCapabilities,
) -> std::io::Result<()> {
    let mut output = output;
    queue!(&mut output, EnterAlternateScreen)?;
    queue!(&mut output, EnableMouseCapture)?;
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

/// Leave the fullscreen surface: mouse reporting off, alternate screen
/// restored, cursor shown, and all capability modes popped.
pub(super) fn write_leave_output(
    output: &mut dyn Write,
    capabilities: TerminalCapabilities,
) -> std::io::Result<()> {
    let mut output = output;
    let mut result = output.write_all(b"\x1b[?25h");
    if let Err(error) = execute!(&mut output, DisableMouseCapture)
        && result.is_ok()
    {
        result = Err(error);
    }
    if let Err(error) = execute!(&mut output, LeaveAlternateScreen)
        && result.is_ok()
    {
        result = Err(error);
    }
    if capabilities.ansi.kitty_keyboard
        && let Err(error) = execute!(&mut output, PopKeyboardEnhancementFlags)
        && result.is_ok()
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

#[derive(Debug)]
pub(super) struct TerminalModeGuard {
    capabilities: TerminalCapabilities,
    active: bool,
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
            #[cfg(windows)]
            windows_input_mode,
        })
    }

    pub(super) fn leave(&mut self) {
        if !self.active {
            return;
        }
        let mut output = stdout();
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

    #[cfg(windows)]
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

        #[cfg(all(test, windows))]
        pub(super) const fn for_test() -> Self {
            Self::inactive()
        }

        #[cfg(windows)]
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

        #[cfg(windows)]
        pub(super) fn restore(&mut self) {
            self.restore_with(set_console_mode);
        }

        fn restore_with(&mut self, set: fn(u32) -> io::Result<()>) {
            if !self.changed {
                return;
            }
            // Only clear `changed` after a successful restore so a failed set
            // leaves the guard able to retry on later leave/Drop.
            if set(self.original_mode).is_ok() {
                self.changed = false;
            }
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

        #[test]
        fn restore_failure_keeps_changed_for_retry() {
            static MODE: AtomicU32 = AtomicU32::new(0x0007);
            static SET_CALLS: AtomicU32 = AtomicU32::new(0);

            fn query() -> io::Result<u32> {
                Ok(MODE.load(Ordering::SeqCst))
            }
            fn set(mode: u32) -> io::Result<()> {
                let calls = SET_CALLS.fetch_add(1, Ordering::SeqCst);
                if calls == 0 {
                    // First set enables VT during enter.
                    MODE.store(mode, Ordering::SeqCst);
                    return Ok(());
                }
                if calls == 1 {
                    // First restore attempt fails.
                    return Err(io::Error::new(
                        io::ErrorKind::PermissionDenied,
                        "restore failed",
                    ));
                }
                MODE.store(mode, Ordering::SeqCst);
                Ok(())
            }

            let original = MODE.load(Ordering::SeqCst);
            let mut guard = WindowsInputModeGuard::enter_with(ConsoleModeOps { query, set })
                .expect("enable VT input mode");
            assert!(guard.changed);

            guard.restore_with(set);
            assert!(
                guard.changed,
                "failed restore must keep changed so Drop/leave can retry"
            );
            assert_eq!(
                MODE.load(Ordering::SeqCst),
                original | ENABLE_VIRTUAL_TERMINAL_INPUT,
                "failed restore must not claim success"
            );

            guard.restore_with(set);
            assert!(!guard.changed);
            assert_eq!(MODE.load(Ordering::SeqCst), original);
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::terminal_capabilities::{AnsiCapabilities, TerminalCapabilities};

    use super::{write_enter_output, write_leave_output};

    #[test]
    fn fullscreen_enter_and_leave_are_single_sequences() {
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
        let enter = String::from_utf8_lossy(&enter);
        let leave = String::from_utf8_lossy(&leave);

        // Exactly one alternate-screen enter and one mouse-capture enable.
        assert_eq!(enter.matches("?1049h").count(), 1);
        assert_eq!(enter.matches("?1000h").count(), 1);
        assert_eq!(enter.matches("?2004h").count(), 1);
        assert!(!enter.contains("?1049l"));
        assert!(!enter.contains("?1000l"));

        // Exactly one leave sequence that restores every entered mode.
        assert_eq!(leave.matches("?1049l").count(), 1);
        assert_eq!(leave.matches("?1000l").count(), 1);
        assert_eq!(leave.matches("?2004l").count(), 1);
        assert!(leave.contains("\x1b[?25h"));
        assert!(!leave.contains("?1049h"));
        assert!(!leave.contains("?1000h"));
        assert!(!enter.contains("\x1b[2J") && !leave.contains("\x1b[2J"));
        assert!(!enter.contains("\x1b[3J") && !leave.contains("\x1b[3J"));
    }
}
