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
    queue!(&mut output, EnterAlternateScreen, EnableMouseCapture)?;
    output.flush()
}

pub(super) fn write_leave_review_output(output: &mut dyn Write) -> std::io::Result<()> {
    let mut output = output;
    queue!(&mut output, DisableMouseCapture, LeaveAlternateScreen)?;
    output.flush()
}

#[derive(Debug)]
pub(super) struct TerminalModeGuard {
    capabilities: TerminalCapabilities,
    active: bool,
    review_active: bool,
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
            #[cfg(windows)]
            windows_input_mode,
        })
    }

    pub(super) fn leave(&mut self) {
        if !self.active {
            if self.review_active {
                let mut output = stdout();
                let _ = write_leave_review_output(&mut output);
                self.review_active = false;
                let _ = output.flush();
            }
            return;
        }
        let mut output = stdout();
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

    #[cfg(test)]
    pub(super) fn for_test() -> Self {
        Self {
            capabilities: TerminalCapabilities::default(),
            active: true,
            review_active: false,
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

#[cfg(windows)]
mod windows_input_mode {
    use std::io;

    const ENABLE_VIRTUAL_TERMINAL_INPUT: u32 = 0x0200;

    #[derive(Debug, Clone, Copy)]
    pub(super) struct WindowsInputModeGuard {
        original_mode: u32,
        changed: bool,
    }

    impl WindowsInputModeGuard {
        fn inactive() -> Self {
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
            let stdin = io::stdin();
            let Ok(mode) = winapi_util::console::mode(&stdin) else {
                return Ok(Self::inactive());
            };
            let vt_mode = mode | ENABLE_VIRTUAL_TERMINAL_INPUT;
            if vt_mode == mode {
                return Ok(Self::inactive());
            }
            winapi_util::console::set_mode(&stdin, vt_mode)?;
            Ok(Self {
                original_mode: mode,
                changed: true,
            })
        }

        pub(super) fn restore(&mut self) {
            if !self.changed {
                return;
            }
            let stdin = io::stdin();
            let _ = winapi_util::console::set_mode(&stdin, self.original_mode);
            self.changed = false;
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
    fn review_modes_are_symmetric() {
        let mut enter = Vec::new();
        write_enter_review_output(&mut enter).expect("review enter output");
        let mut leave = Vec::new();
        write_leave_review_output(&mut leave).expect("review leave output");

        let enter = String::from_utf8(enter).expect("review enter is UTF-8");
        let leave = String::from_utf8(leave).expect("review leave is UTF-8");
        assert!(enter.contains("?1049h"));
        assert!(leave.contains("?1049l"));
        assert!(enter.contains("?1000h"));
        assert!(leave.contains("?1000l"));
        assert!(!format!("{enter}{leave}").contains("\x1b[2J"));
        assert!(!format!("{enter}{leave}").contains("\x1b[3J"));
        let mouse_off = leave.find("?1000l").expect("mouse capture disabled");
        let alternate_off = leave.find("?1049l").expect("alternate screen left");
        assert!(mouse_off < alternate_off);
    }
}
