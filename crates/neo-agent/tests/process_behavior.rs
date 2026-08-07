//! Process behavior: Unix process-guard trees, Windows job objects,
//! shell admission queueing, and bash/terminal guardian lifetimes.

#[path = "process_behavior/bash_guardian.rs"]
mod bash_guardian;
#[path = "process_behavior/process_guard_unix.rs"]
mod process_guard_unix;
#[path = "process_behavior/process_guard_windows.rs"]
mod process_guard_windows;
#[path = "process_behavior/shell_admission.rs"]
mod shell_admission;
#[path = "process_behavior/terminal_guardian.rs"]
mod terminal_guardian;
#[path = "process_behavior/terminal_guardian_capture.rs"]
mod terminal_guardian_capture;
