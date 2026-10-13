const UNITS: [(&str, u64); 5] = [
    ("d", 86_400_000),
    ("h", 3_600_000),
    ("m", 60_000),
    ("s", 1_000),
    ("ms", 1),
];

pub fn format_duration(seconds: &str) -> Option<String> {
    let seconds = match seconds.trim().parse::<f64>() {
        Ok(seconds) if seconds.is_finite() && seconds >= 0.0 => seconds,
        _ => return None,
    };

    let mut rest = (seconds * 1000.0).round() as u64;
    let mut parts = Vec::new();
    for (unit, unit_ms) in UNITS {
        let count = rest / unit_ms;
        rest %= unit_ms;
        if count > 0 {
            parts.push(format!("{}{}", count, unit));
        }
    }
    if parts.is_empty() {
        parts.push(String::from("0s"));
    }

    Some(parts.join(" "))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn whole_seconds_render_as_space_separated_units() {
        assert_eq!(format_duration("3600").as_deref(), Some("1h"));
        assert_eq!(format_duration("146").as_deref(), Some("2m 26s"));
        assert_eq!(format_duration("90061").as_deref(), Some("1d 1h 1m 1s"));
        assert_eq!(format_duration("0").as_deref(), Some("0s"));
    }

    #[test]
    fn fractions_show_as_milliseconds() {
        assert_eq!(format_duration("1.5").as_deref(), Some("1s 500ms"));
        assert_eq!(format_duration("0.25").as_deref(), Some("250ms"));
        assert_eq!(format_duration("61.0").as_deref(), Some("1m 1s"));
        assert_eq!(format_duration(" 2 ").as_deref(), Some("2s"));
    }

    #[test]
    fn unrenderable_input_yields_none() {
        assert_eq!(format_duration(""), None);
        assert_eq!(format_duration("abc"), None);
        assert_eq!(format_duration("-5"), None);
        assert_eq!(format_duration("inf"), None);
        assert_eq!(format_duration("NaN"), None);
    }
}
