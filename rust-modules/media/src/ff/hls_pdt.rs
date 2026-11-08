//! Strict PROGRAM-DATE-TIME parsing for the independent HLS presentation layer.
//! Produces an absolute Unix timestamp only; this does not establish A/V sync.

const INVALID: &str = "Invalid HLS program date/time";
const OUT_OF_RANGE: &str = "HLS program date/time exceeds nanosecond range";

/// Parse the RFC 3339 timestamp shape used by EXT-X-PROGRAM-DATE-TIME.
///
/// The date uses four year digits and the proleptic Gregorian calendar. Both
/// RFC 3339 cases of T/Z are accepted; whitespace, leap seconds, a bare decimal
/// point and fractions longer than nine digits are rejected. Fractions are
/// optional and, when present, have one through nine digits. Numeric offsets
/// range from -23:59 through +23:59. `-00:00` converts to the same UTC instant
/// as `Z`; its unknown-local-offset annotation is not retained by this helper.
///
/// Arithmetic remains wider than i64 until the offset and fractional part have
/// been applied, so timestamps at either i64 boundary work with nonzero offsets.
pub(crate) fn parse_program_date_time(value: &str) -> Result<i64, &'static str> {
    let bytes = value.as_bytes();
    if !(20..=35).contains(&bytes.len())
        || bytes[4] != b'-'
        || bytes[7] != b'-'
        || !matches!(bytes[10], b'T' | b't')
        || bytes[13] != b':'
        || bytes[16] != b':'
    {
        return Err(INVALID);
    }

    let year = decimal(bytes, 0, 4)?;
    let month = decimal(bytes, 5, 2)?;
    let day = decimal(bytes, 8, 2)?;
    let hour = decimal(bytes, 11, 2)?;
    let minute = decimal(bytes, 14, 2)?;
    let second = decimal(bytes, 17, 2)?;
    if !(1..=12).contains(&month) || hour > 23 || minute > 59 || second > 59 {
        return Err(INVALID);
    }
    let leap = is_leap_year(year);
    let month_days = match month {
        2 if leap => 29,
        2 => 28,
        4 | 6 | 9 | 11 => 30,
        _ => 31,
    };
    if day == 0 || day > month_days {
        return Err(INVALID);
    }

    let mut at = 19;
    let mut fraction_ns = 0i128;
    if bytes.get(at) == Some(&b'.') {
        at += 1;
        let start = at;
        while bytes.get(at).is_some_and(u8::is_ascii_digit) {
            if at - start == 9 {
                return Err(INVALID);
            }
            fraction_ns = fraction_ns * 10 + i128::from(bytes[at] - b'0');
            at += 1;
        }
        let digits = at - start;
        if digits == 0 {
            return Err(INVALID);
        }
        for _ in digits..9 {
            fraction_ns *= 10;
        }
    }

    let offset_seconds = match bytes.get(at) {
        Some(b'Z' | b'z') if at + 1 == bytes.len() => 0i128,
        Some(sign @ (b'+' | b'-')) if at + 6 == bytes.len() && bytes[at + 3] == b':' => {
            let offset_hour = decimal(bytes, at + 1, 2)?;
            let offset_minute = decimal(bytes, at + 4, 2)?;
            if offset_hour > 23 || offset_minute > 59 {
                return Err(INVALID);
            }
            let offset = i128::from(offset_hour) * 3_600 + i128::from(offset_minute) * 60;
            if *sign == b'-' { -offset } else { offset }
        }
        _ => return Err(INVALID),
    };

    const BEFORE_MONTH: [i128; 12] = [0, 31, 59, 90, 120, 151, 181, 212, 243, 273, 304, 334];
    let days = days_before_year(year) - days_before_year(1970)
        + BEFORE_MONTH[(month - 1) as usize]
        + i128::from(u8::from(leap && month > 2))
        + i128::from(day - 1);
    // Four-digit years and bounded fields make all intermediate values fit in
    // i128. Check the requested output range only after translating to UTC.
    let seconds = days * 86_400 + i128::from(hour) * 3_600
        + i128::from(minute) * 60 + i128::from(second) - offset_seconds;
    let nanos = seconds * 1_000_000_000 + fraction_ns;
    i64::try_from(nanos).map_err(|_| OUT_OF_RANGE)
}

fn decimal(bytes: &[u8], start: usize, len: usize) -> Result<u32, &'static str> {
    let digits = bytes.get(start..start + len).ok_or(INVALID)?;
    let mut value = 0u32;
    for digit in digits {
        if !digit.is_ascii_digit() {
            return Err(INVALID);
        }
        value = value * 10 + u32::from(*digit - b'0');
    }
    Ok(value)
}

fn is_leap_year(year: u32) -> bool {
    year % 4 == 0 && (year % 100 != 0 || year % 400 == 0)
}

fn days_before_year(year: u32) -> i128 {
    let previous = i128::from(year) - 1;
    previous * 365 + previous.div_euclid(4) - previous.div_euclid(100)
        + previous.div_euclid(400)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn canonical_utc_and_numeric_offsets_are_the_same_instant() {
        for value in [
            "1970-01-01T00:00:00Z",
            "1970-01-01t00:00:00z",
            "1970-01-01T00:00:00+00:00",
            "1970-01-01T00:00:00-00:00",
            "1970-01-01T01:30:00+01:30",
            "1969-12-31T18:30:00-05:30",
        ] {
            assert_eq!(parse_program_date_time(value), Ok(0), "{value}");
        }
        assert_eq!(parse_program_date_time("1970-01-02T00:00:00+23:59"), Ok(60_000_000_000));
        assert_eq!(parse_program_date_time("1969-12-31T00:00:00-23:59"), Ok(-60_000_000_000));
    }

    #[test]
    fn fractions_have_exact_nanosecond_precision_on_both_sides_of_epoch() {
        let cases = [
            ("1970-01-01T00:00:00.1Z", 100_000_000),
            ("1970-01-01T00:00:00.01Z", 10_000_000),
            ("1970-01-01T00:00:00.123456Z", 123_456_000),
            ("1970-01-01T00:00:00.123456789Z", 123_456_789),
            ("1970-01-01T00:00:00.000000001Z", 1),
            ("1969-12-31T23:59:59.999999999Z", -1),
            ("1970-01-01T01:00:00.000000001+01:00", 1),
        ];
        for (value, nanos) in cases {
            assert_eq!(parse_program_date_time(value), Ok(nanos), "{value}");
        }
    }

    #[test]
    fn leap_years_centuries_and_month_boundaries_follow_gregorian_rules() {
        let day_ns = 86_400_000_000_000;
        let parse = |value| parse_program_date_time(value).unwrap();
        assert_eq!(parse("2000-02-29T00:00:00Z") - parse("2000-02-28T00:00:00Z"), day_ns);
        assert_eq!(parse("2000-03-01T00:00:00Z") - parse("2000-02-29T00:00:00Z"), day_ns);
        assert_eq!(parse("2001-03-01T00:00:00Z") - parse("2001-02-28T00:00:00Z"), day_ns);
        assert_eq!(parse("2001-05-01T00:00:00Z") - parse("2001-04-30T00:00:00Z"), day_ns);
        assert_eq!(parse("2001-01-01T00:00:00Z") - parse("2000-12-31T00:00:00Z"), day_ns);
        assert!(parse_program_date_time("2004-02-29T12:34:56Z").is_ok());
        for value in [
            "1900-02-29T00:00:00Z", "2001-02-29T00:00:00Z", "2100-02-29T00:00:00Z",
            "2000-02-30T00:00:00Z", "2001-04-31T00:00:00Z", "2001-06-31T00:00:00Z",
            "2001-09-31T00:00:00Z", "2001-11-31T00:00:00Z", "2001-00-01T00:00:00Z",
            "2001-13-01T00:00:00Z", "2001-01-00T00:00:00Z", "2001-01-32T00:00:00Z",
        ] {
            assert_eq!(parse_program_date_time(value), Err(INVALID), "{value}");
        }
    }

    #[test]
    fn malformed_incomplete_and_non_rfc3339_forms_are_rejected() {
        for value in [
            "", "1970", "1970-01-01", "1970-01-01T00:00:00", "1970-01-01T00:00:00.",
            "1970-01-01T00:00:00.Z", "1970-01-01T00:00:00.1234567890Z",
            "1970-01-01T00:00:00+", "1970-01-01T00:00:00+01", "1970-01-01T00:00:00+01:",
            "1970-01-01T00:00:00+01:0", "1970-01-01T00:00:00+0100",
            "1970-01-01T00:00:00+01:00:00", "1970-01-01T00:00:00+24:00",
            "1970-01-01T00:00:00-24:00", "1970-01-01T00:00:00+01:60",
            "1970-01-01T00:00:00Zx", "1970-01-01T00:00:00Z\n", " 1970-01-01T00:00:00Z",
            "1970-01-01 00:00:00Z", "1970/01/01T00:00:00Z", "1970-1-01T00:00:00Z",
            "1970-01-01T24:00:00Z", "1970-01-01T00:60:00Z", "1970-01-01T00:00:61Z",
            "2016-12-31T23:59:60Z", "1970-01-01T00:00:00,1Z",
            "1970-01-01T00:00:00.1e3Z", "1970-01-01T00:00:00+aa:00",
            "1970-01-01T00:00:00UTC", "１９７０-01-01T00:00:00Z",
        ] {
            assert_eq!(parse_program_date_time(value), Err(INVALID), "{value}");
        }
    }

    #[test]
    fn exact_i64_boundaries_are_checked_after_offset_and_fraction() {
        for value in [
            "1677-09-21T00:12:43.145224192Z",
            "1677-09-20T23:12:43.145224192-01:00",
        ] {
            assert_eq!(parse_program_date_time(value), Ok(i64::MIN), "{value}");
        }
        for value in [
            "2262-04-11T23:47:16.854775807Z",
            "2262-04-12T00:47:16.854775807+01:00",
        ] {
            assert_eq!(parse_program_date_time(value), Ok(i64::MAX), "{value}");
        }
        for value in [
            "1677-09-21T00:12:43.145224191Z",
            "2262-04-11T23:47:16.854775808Z",
            "1677-09-21T00:12:43.145224192+00:01",
            "2262-04-11T23:47:16.854775807-00:01",
            "0000-01-01T00:00:00Z", "9999-12-31T23:59:59.999999999Z",
        ] {
            assert_eq!(parse_program_date_time(value), Err(OUT_OF_RANGE), "{value}");
        }
    }
}
