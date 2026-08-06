use super::*;
use std::time::Duration;

#[test]
fn parse_retry_after_maps_delta_seconds_and_http_dates() {
    // (case, input, expected)
    let cases = [
        ("delta seconds", "30", Some(Duration::from_secs(30))),
        (
            "padded delta seconds",
            "  5  ",
            Some(Duration::from_secs(5)),
        ),
        (
            "past http date",
            "Sun, 06 Nov 1994 08:49:37 GMT",
            Some(Duration::ZERO),
        ),
        ("non numeric text", "not a number", None),
        ("empty string", "", None),
    ];
    for (name, input, expected) in cases {
        assert_eq!(parse_retry_after(input), expected, "case {name}");
    }
}
