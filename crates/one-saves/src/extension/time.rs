//! Calendar dates, for showing an instant to a person and for nothing else.

use std::fmt;

/// Whole seconds on the Unix scale, shown as `YYYY-MM-DD HH:MM:SS`.
///
/// No zone is printed, because what the seconds mean is the caller's to say: an `x.1sav.rtc`
/// reading is an instant, where an `x.1sav.dirent` one is a wall clock read as though it were UTC.
pub(super) struct Civil(pub i64);

impl fmt::Display for Civil {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let (year, month, day) = civil_from_days(self.0.div_euclid(86_400));
        let second_of_day = self.0.rem_euclid(86_400);
        let (hour, minute, second) = (second_of_day / 3_600, second_of_day / 60 % 60, second_of_day % 60);
        write!(f, "{year:04}-{month:02}-{day:02} {hour:02}:{minute:02}:{second:02}")
    }
}

/// The civil date a count of days from 1970-01-01 falls on, by Howard Hinnant's algorithm.
fn civil_from_days(days: i64) -> (i64, i64, i64) {
    let days = days + 719_468;
    let era = if days >= 0 { days } else { days - 146_096 } / 146_097;
    let day_of_era = days - era * 146_097;
    let year_of_era = (day_of_era - day_of_era / 1_460 + day_of_era / 36_524 - day_of_era / 146_096) / 365;
    let day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
    let month_index = (5 * day_of_year + 2) / 153;
    let day = day_of_year - (153 * month_index + 2) / 5 + 1;
    let month = if month_index < 10 { month_index + 3 } else { month_index - 9 };
    (year_of_era + era * 400 + i64::from(month <= 2), month, day)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shows_the_date_an_instant_falls_on() {
        assert_eq!(Civil(0).to_string(), "1970-01-01 00:00:00");
        assert_eq!(Civil(1_700_000_000).to_string(), "2023-11-14 22:13:20");
        // A leap day, and the second before the epoch, which is where truncating division goes wrong.
        assert_eq!(Civil(951_782_400).to_string(), "2000-02-29 00:00:00");
        assert_eq!(Civil(-1).to_string(), "1969-12-31 23:59:59");
    }
}
