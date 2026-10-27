use movie_match::catalog::{released_between, with_min_votes, ReleaseWindows};
use movie_match::dates::Date;
use movie_match::tmdb::MovieDetails;

fn date(year: i32, month: u32, day: u32) -> Date {
    Date { year, month, day }
}

#[test]
fn unix_days_become_calendar_dates() {
    assert_eq!(Date::from_unix_days(0), date(1970, 1, 1));
    assert_eq!(Date::from_unix_days(-1), date(1969, 12, 31));
    // 2000 was a leap year (divisible by 400), 1900 wasn't.
    assert_eq!(Date::from_unix_days(11_016), date(2000, 2, 29));
    assert_eq!(Date::from_unix_days(20_752), date(2026, 10, 26));
    assert_eq!(date(2026, 3, 5).to_string(), "2026-03-05");
}

#[test]
fn unix_days_round_trip() {
    for days in [-800_000, -1, 0, 59, 60, 11_016, 20_752, 3_000_000] {
        assert_eq!(Date::from_unix_days(days).to_unix_days(), days, "{days}");
    }
}

#[test]
fn months_earlier_clamps_to_the_month_length() {
    assert_eq!(date(2026, 10, 26).months_earlier(12), date(2025, 10, 26));
    assert_eq!(date(2026, 3, 31).months_earlier(1), date(2026, 2, 28));
    assert_eq!(date(2024, 3, 31).months_earlier(1), date(2024, 2, 29));
    assert_eq!(date(2026, 1, 15).months_earlier(2), date(2025, 11, 15));
}

#[test]
fn windows_split_at_today() {
    // 2026-10-26, with a 6-month recent window.
    let windows = ReleaseWindows::new(20_752, 6);
    assert_eq!(windows.upcoming_from, "2026-10-26");
    assert_eq!(windows.recent_to, "2026-10-25");
    assert_eq!(windows.recent_from, "2026-04-26");
}

#[test]
fn recent_slices_cover_the_window_newest_first() {
    let slice = |from: &str, to: &str| (from.to_string(), to.to_string());
    // 2026-10-26, with a 7-month window: two 3-month slices and a 1-month remainder.
    let windows = ReleaseWindows::new(20_752, 7);
    assert_eq!(
        windows.recent_slices,
        [
            slice("2026-07-26", "2026-10-25"),
            slice("2026-04-26", "2026-07-25"),
            slice("2026-03-26", "2026-04-25"),
        ]
    );
    assert_eq!(windows.recent_slices.last().unwrap().0, windows.recent_from);
}

#[test]
fn recent_slices_end_before_a_clamped_start() {
    // 2026-05-31: three months back is 28 February, so the older slice ends the 27th.
    let windows = ReleaseWindows::new(date(2026, 5, 31).to_unix_days(), 6);
    let slice = |from: &str, to: &str| (from.to_string(), to.to_string());
    assert_eq!(
        windows.recent_slices,
        [
            slice("2026-02-28", "2026-05-30"),
            slice("2025-11-30", "2026-02-27"),
        ]
    );
}

#[test]
fn min_votes_filter_treats_missing_counts_as_zero() {
    let voted = |id: u64, votes: Option<u32>| MovieDetails {
        id,
        vote_count: votes,
        ..MovieDetails::default()
    };
    let movies = vec![voted(1, Some(50)), voted(2, Some(49)), voted(3, None)];
    let ids: Vec<u64> = with_min_votes(movies.clone(), 50)
        .into_iter()
        .map(|m| m.id)
        .collect();
    assert_eq!(ids, [1]);
    assert_eq!(with_min_votes(movies, 0).len(), 3);
}

#[test]
fn release_date_filter_keeps_only_dated_movies_in_range() {
    let dated = |id: u64, date: &str| MovieDetails {
        id,
        release_date: date.to_string(),
        ..MovieDetails::default()
    };
    let movies = vec![
        dated(1, "2026-10-26"),
        dated(2, "2026-10-25"),
        dated(3, "2025-01-01"),
        dated(4, ""),
        dated(5, "2027-06-01"),
    ];
    let ids = |list: Vec<MovieDetails>| list.into_iter().map(|m| m.id).collect::<Vec<_>>();

    // Upcoming: today or later.
    assert_eq!(
        ids(released_between(movies.clone(), "2026-10-26", None)),
        [1, 5]
    );
    // Recent: a window that ends yesterday.
    assert_eq!(
        ids(released_between(movies, "2026-04-26", Some("2026-10-25"))),
        [2]
    );
}
