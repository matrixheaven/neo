use super::*;

#[test]
fn format_exit_code_success() {
    assert_eq!(format_exit_code(Some(0), None), "0");
}

#[test]
fn format_exit_code_nonzero() {
    assert_eq!(format_exit_code(Some(1), None), "1");
    assert_eq!(format_exit_code(Some(127), None), "127");
}

#[test]
fn format_exit_code_signal_only() {
    assert_eq!(format_exit_code(None, Some(9)), "signal 9");
}

#[test]
fn format_exit_code_code_and_signal() {
    assert_eq!(format_exit_code(Some(1), Some(9)), "1 (signal 9)");
}

#[test]
fn format_exit_code_no_code_no_signal() {
    assert_eq!(format_exit_code(None, None), "no exit code");
}
