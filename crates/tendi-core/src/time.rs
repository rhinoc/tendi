use std::cmp::Ordering;

use chrono::{DateTime, FixedOffset};

pub(crate) fn parse_timestamp(value: &str) -> Option<DateTime<FixedOffset>> {
    DateTime::parse_from_rfc3339(value.trim()).ok()
}

pub(crate) fn timestamp_ms(value: &str) -> Option<i64> {
    parse_timestamp(value).map(|timestamp| timestamp.timestamp_millis())
}

pub(crate) fn compare_timestamps(left: Option<&str>, right: Option<&str>) -> Ordering {
    match (left, right) {
        (Some(left), Some(right)) => match (parse_timestamp(left), parse_timestamp(right)) {
            (Some(left), Some(right)) => left.cmp(&right),
            (Some(_), None) => Ordering::Greater,
            (None, Some(_)) => Ordering::Less,
            (None, None) => left.cmp(right),
        },
        (left, right) => left.cmp(&right),
    }
}
