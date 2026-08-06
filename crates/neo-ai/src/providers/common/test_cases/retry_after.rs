use super::*;
use std::time::Duration;

#[test]
fn parse_retry_after_seconds() {
    assert_eq!(parse_retry_after("30"), Some(Duration::from_secs(30)));
    assert_eq!(parse_retry_after("  5  "), Some(Duration::from_secs(5)));
}

#[test]
fn parse_retry_after_past_http_date_returns_zero() {
    assert_eq!(
        parse_retry_after("Sun, 06 Nov 1994 08:49:37 GMT"),
        Some(Duration::ZERO)
    );
}

#[test]
fn parse_retry_after_invalid_returns_none() {
    assert_eq!(parse_retry_after("not a number"), None);
    assert_eq!(parse_retry_after(""), None);
}
