//! Calendar dates as TMDB writes them (`YYYY-MM-DD`), without a date library. Pure:
//! the caller supplies "today" as days since 1970-01-01.

use std::fmt;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct Date {
    pub year: i32,
    pub month: u32,
    pub day: u32,
}

impl Date {
    /// The date `days` days after 1970-01-01 (negative for earlier), in the
    /// proleptic Gregorian calendar.
    pub fn from_unix_days(days: i64) -> Self {
        // Howard Hinnant's civil_from_days: count from 0000-03-01 so leap days fall
        // at the end of each 400-year era.
        let z = days + 719_468;
        let era = z.div_euclid(146_097);
        let day_of_era = z.rem_euclid(146_097);
        let year_of_era =
            (day_of_era - day_of_era / 1460 + day_of_era / 36_524 - day_of_era / 146_096) / 365;
        let day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
        let month_index = (5 * day_of_year + 2) / 153; // 0 = March
        let day = (day_of_year - (153 * month_index + 2) / 5 + 1) as u32;
        let month = if month_index < 10 {
            month_index + 3
        } else {
            month_index - 9
        } as u32;
        let year = (year_of_era + era * 400 + i64::from(month <= 2)) as i32;
        Self { year, month, day }
    }

    /// Days from 1970-01-01 to this date (negative before it): the inverse of
    /// [`Date::from_unix_days`].
    pub fn to_unix_days(self) -> i64 {
        // Howard Hinnant's days_from_civil, counting from 0000-03-01.
        let year = i64::from(self.year) - i64::from(self.month <= 2);
        let era = year.div_euclid(400);
        let year_of_era = year.rem_euclid(400);
        let month = i64::from(self.month);
        let month_index = if month > 2 { month - 3 } else { month + 9 };
        let day_of_year = (153 * month_index + 2) / 5 + i64::from(self.day) - 1;
        let day_of_era = year_of_era * 365 + year_of_era / 4 - year_of_era / 100 + day_of_year;
        era * 146_097 + day_of_era - 719_468
    }

    /// The same day `months` months earlier, moved back to the month's last day
    /// when it has fewer days (e.g. 31 March minus one month is 28 or 29 February).
    pub fn months_earlier(self, months: u32) -> Self {
        let total = self.year * 12 + self.month as i32 - 1 - months as i32;
        let year = total.div_euclid(12);
        let month = total.rem_euclid(12) as u32 + 1;
        Self {
            year,
            month,
            day: self.day.min(days_in_month(year, month)),
        }
    }
}

impl fmt::Display for Date {
    /// `YYYY-MM-DD`, which also sorts correctly as text.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{:04}-{:02}-{:02}", self.year, self.month, self.day)
    }
}

fn days_in_month(year: i32, month: u32) -> u32 {
    match month {
        2 if year % 4 == 0 && (year % 100 != 0 || year % 400 == 0) => 29,
        2 => 28,
        4 | 6 | 9 | 11 => 30,
        _ => 31,
    }
}
