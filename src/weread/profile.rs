use super::{integer, Error, Result};
use serde_json::Value;

fn seconds(value: &Value, key: &str) -> Result<u64> {
    value
        .get(key)
        .and_then(integer)
        .and_then(|v| v.try_into().ok())
        .ok_or_else(|| Error::new(format!("Service response has no valid {key}.")))
}

pub(super) fn parse_reading_time(value: &Value) -> Result<u64> {
    seconds(value, "totalReadingTime")
}

pub(super) fn parse_weekly_reading_time(value: &Value) -> Result<u64> {
    // /readdata/detail uses totalReadTime, unlike /user/profile.
    seconds(value, "totalReadTime")
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn durations_are_seconds_and_unknown_fields_are_not_zero() {
        assert_eq!(
            parse_reading_time(&json!({"totalReadingTime":"3661"})).unwrap(),
            3661
        );
        assert_eq!(
            parse_reading_time(&json!({"totalReadingTime":0})).unwrap(),
            0
        );
        for value in [
            json!({}),
            json!({"totalReadingTime":null}),
            json!({"totalReadingTime":-1}),
            json!({"totalReadingTime":1.5}),
        ] {
            assert!(parse_reading_time(&value).is_err());
        }
    }

    #[test]
    fn weekly_time_uses_the_period_total_in_seconds() {
        for (value, seconds) in [
            (json!({"totalReadTime":0}), 0),
            (json!({"totalReadTime":"5400"}), 5400),
        ] {
            assert_eq!(parse_weekly_reading_time(&value).unwrap(), seconds);
        }
        for value in [
            json!({}),
            json!({"totalReadingTime":5400}),
            json!({"totalReadTime":null}),
            json!({"totalReadTime":-1}),
            json!({"totalReadTime":1.5}),
        ] {
            assert!(parse_weekly_reading_time(&value).is_err());
        }
    }
}
