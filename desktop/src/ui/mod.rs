//! The view layer (PLAN §5): one module per region of the window, plus the formatting
//! helpers they share.
//!
//! Nothing here holds state or decides anything — a view function takes a borrow of the
//! model and returns an `Element`. The helpers below are the exception worth testing:
//! they are small pure string functions, and getting a date or a duration subtly wrong is
//! the kind of bug that survives a hundred glances at the screen.

pub mod browser;
pub mod deck;
pub mod mixer;
pub mod tree;
pub mod waveform;

use std::time::{Duration, SystemTime, UNIX_EPOCH};

/// Shorten a string to fit, cutting the middle rather than the end.
///
/// The end of a filename is where the disambiguating part usually lives — the track
/// number is at the front and the extension at the back, so an ellipsis in the middle
/// keeps both.
pub fn elide_middle(text: &str, max_chars: usize) -> String {
	let chars: Vec<char> = text.chars().collect();
	if chars.len() <= max_chars {
		return text.to_string();
	}
	if max_chars <= 1 {
		return "…".to_string();
	}

	// One character goes to the ellipsis; the leading half gets the odd one, because the
	// front of a name carries more information than the back.
	let keep = max_chars - 1;
	let head = keep.div_ceil(2);
	let tail = keep - head;

	let mut out: String = chars[..head].iter().collect();
	out.push('…');
	out.extend(&chars[chars.len() - tail..]);
	out
}

/// A file size for a browser row: three significant figures at most, so the column stays
/// narrow and the numbers stay comparable at a glance.
pub fn format_size(bytes: u64) -> String {
	const UNITS: [&str; 5] = ["B", "KB", "MB", "GB", "TB"];

	let mut value = bytes as f64;
	let mut unit = 0;
	while value >= 1024.0 && unit + 1 < UNITS.len() {
		value /= 1024.0;
		unit += 1;
	}

	if unit == 0 {
		format!("{bytes} B")
	} else if value < 10.0 {
		format!("{value:.1} {}", UNITS[unit])
	} else {
		format!("{value:.0} {}", UNITS[unit])
	}
}

/// A modification time as `YYYY-MM-DD`.
///
/// `ponytail:` UTC, not local time, because a timezone database is a dependency and
/// `std` has none. A file saved late in the evening can therefore show tomorrow's date
/// west of Greenwich. Upgrade path is a `jiff`/`time` dependency, worth it only if the
/// date column starts being used for something other than "recent or not".
pub fn format_date(time: Option<SystemTime>) -> String {
	let Some(time) = time else {
		return String::new();
	};
	let Ok(since_epoch) = time.duration_since(UNIX_EPOCH) else {
		// Before 1970. Possible on a filesystem with a broken clock, not worth rendering.
		return String::new();
	};

	let days = (since_epoch.as_secs() / 86_400) as i64;
	let (year, month, day) = civil_from_days(days);
	format!("{year:04}-{month:02}-{day:02}")
}

/// Days since the Unix epoch to a calendar date, by Howard Hinnant's `civil_from_days`.
///
/// Written out rather than pulled in: it is fifteen lines of integer arithmetic with no
/// timezone, no leap seconds and no locale, which is exactly the amount of calendar this
/// app needs.
fn civil_from_days(days: i64) -> (i64, u32, u32) {
	// Shift the epoch to 0000-03-01 so leap days land at the end of the year and the
	// month arithmetic below has no special case for February.
	let shifted = days + 719_468;
	let era = if shifted >= 0 {
		shifted
	} else {
		shifted - 146_096
	} / 146_097;
	let day_of_era = shifted - era * 146_097; // [0, 146096]
	let year_of_era =
		(day_of_era - day_of_era / 1460 + day_of_era / 36_524 - day_of_era / 146_096) / 365; // [0, 399]
	let year = year_of_era + era * 400;
	let day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100); // [0, 365]
	let shifted_month = (5 * day_of_year + 2) / 153; // [0, 11], March = 0
	let day = (day_of_year - (153 * shifted_month + 2) / 5 + 1) as u32; // [1, 31]
	let month = if shifted_month < 10 {
		shifted_month + 3
	} else {
		shifted_month - 9
	} as u32;

	(year + i64::from(month <= 2), month, day)
}

/// A playhead or a track length as `M:SS`, or `H:MM:SS` once there is an hour to show.
pub fn format_clock(duration: Duration) -> String {
	let total = duration.as_secs();
	let (hours, minutes, seconds) = (total / 3600, (total % 3600) / 60, total % 60);

	if hours > 0 {
		format!("{hours}:{minutes:02}:{seconds:02}")
	} else {
		format!("{minutes}:{seconds:02}")
	}
}

/// The `position / length` readout, with the length replaced when the decoder could not
/// determine one (PLAN §7).
pub fn format_transport(position: Duration, duration: Option<Duration>) -> String {
	match duration {
		Some(duration) => format!("{} / {}", format_clock(position), format_clock(duration)),
		None => format!("{} / --:--", format_clock(position)),
	}
}

#[cfg(test)]
mod tests {
	use super::*;

	#[test]
	fn eliding_keeps_both_ends_of_a_name() {
		// Arrange / Act / Assert: the extension survives, which is the point.
		assert_eq!(elide_middle("short.mp3", 20), "short.mp3", "already fits");
		assert_eq!(
			elide_middle("abcdefghij", 5),
			"ab…ij",
			"even budget splits evenly"
		);
		assert_eq!(
			elide_middle("abcdefghij", 6),
			"abc…ij",
			"odd budget favours the front"
		);
		assert_eq!(
			elide_middle("abcdefghij", 1),
			"…",
			"no room for anything else"
		);
	}

	#[test]
	fn eliding_never_exceeds_the_budget() {
		// Arrange: a name longer than every budget tried.
		let name = "01 - a rather long track title.flac";

		// Act / Assert
		for max in 1..=name.chars().count() {
			let elided = elide_middle(name, max);
			assert!(
				elided.chars().count() <= max,
				"{max} chars: got {} in {elided:?}",
				elided.chars().count()
			);
		}
	}

	#[test]
	fn sizes_read_as_three_significant_figures() {
		// Arrange / Act / Assert
		assert_eq!(format_size(0), "0 B");
		assert_eq!(format_size(999), "999 B");
		assert_eq!(format_size(1024), "1.0 KB");
		assert_eq!(format_size(8_598_323), "8.2 MB");
		assert_eq!(format_size(43_000_000), "41 MB");
	}

	#[test]
	fn the_calendar_survives_leap_years_and_the_epoch() {
		// Arrange / Act / Assert: day counts checked against a real calendar, including
		// a leap day, which is the case the shifted-epoch arithmetic exists to get right.
		let at_day = |days: u64| format_date(Some(UNIX_EPOCH + Duration::from_secs(days * 86_400)));

		assert_eq!(at_day(0), "1970-01-01", "the epoch itself");
		assert_eq!(at_day(19_782), "2024-02-29", "a leap day");
		assert_eq!(at_day(20_667), "2026-08-02");
	}

	#[test]
	fn a_missing_modification_time_renders_as_nothing() {
		// Arrange / Act / Assert: a blank cell, not the word "unknown" — the column is
		// scanned, and a filesystem that will not answer is not news.
		assert_eq!(format_date(None), "");
	}

	#[test]
	fn the_clock_grows_an_hours_field_only_when_it_needs_one() {
		// Arrange / Act / Assert
		assert_eq!(format_clock(Duration::from_secs(0)), "0:00");
		assert_eq!(format_clock(Duration::from_secs(42)), "0:42");
		assert_eq!(format_clock(Duration::from_secs(195)), "3:15");
		assert_eq!(format_clock(Duration::from_secs(3_725)), "1:02:05");
	}

	#[test]
	fn a_track_of_unknown_length_still_shows_its_playhead() {
		// Arrange / Act / Assert: the case PLAN §7 warns about — a stream the decoder
		// cannot measure must not blank the whole readout.
		assert_eq!(
			format_transport(Duration::from_secs(42), None),
			"0:42 / --:--"
		);
	}
}
