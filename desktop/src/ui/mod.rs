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
pub mod playlist;
pub mod tree;
pub mod waveform;

use std::ops::Range;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

/// How many rows a scrolling list builds per frame, however long it is (PLAN §9).
///
/// A window is clamped to 4096 points tall (`settings.rs`), so no pane can show more than
/// 171 rows of the files pane's 24 pixels or 187 of a queue's 22; 200 covers both with room
/// to spare for a scroll offset that is a frame stale. A fixed count rather than a measured
/// pane height is what keeps this to *one* number of state — and the margin is nearly free,
/// since the whole point is that 200 rows cost the same whether the list holds 300 or 30 000.
pub const ROWS_BUILT: usize = 200;

/// The gap between a scrolling list's content and its scrollbar, in pixels.
///
/// iced draws a vertical scrollbar *over* the content unless a `scrollable` is given a
/// spacing, and then reserves `width + 2 × margin + spacing` for it — but only while the bar
/// is actually showing, so a short list keeps its full width. Without it the right-hand end of
/// every row sits under the bar, which is invisible until a column is flush with that edge: a
/// queue's running time is, and was being cut in half by it (PLAN §9).
///
/// Small, because the three queue panels share the width two players had and the bar itself is
/// already ten of those pixels.
pub const SCROLLBAR_GAP: f32 = 2.0;

/// Which rows are worth building, for a list scrolled this far down (PLAN §9).
///
/// The rest of the list is two blank blocks of exactly the right height, so the scrollbar is
/// the size it would have been and every row is where it would have been. `row_height` is the
/// pitch from one row to the next, which is not always the row itself: a queue reserves an
/// insertion caret above each of its rows, and the pair moves as one.
///
/// Shared by the files pane and the three queues, because a range that is wrong by one row
/// leaves a blank strip where a row should be and being wrong in two places is worse than
/// being wrong in one.
pub fn visible_rows(scroll: f32, total: usize, row_height: f32, built: usize) -> Range<usize> {
	// The last window that still fills the pane. Clamping to it means a scroll offset left
	// over from a longer listing shows the *end* of the new one rather than a blank pane,
	// which is exactly what the `scrollable` itself does when its content shrinks.
	let last = total.saturating_sub(built);

	// `as usize` saturates rather than wrapping: a negative offset lands on 0, and so does a
	// `NaN`. Worth relying on here, because this number indexes a list.
	let start = ((scroll / row_height) as usize).min(last);

	start..(start + built).min(total)
}

/// The turning glyph for a row whose file is being decoded (PLAN §11c).
///
/// Four frames rather than a braille spinner's eight: the phase comes off the same counter the
/// players' strips sweep on, which takes 1.2 s to go round, and eight frames of 150 ms read as
/// a flicker where four of 300 ms read as turning.
const SPINNER: [&str; 4] = ["◐", "◓", "◑", "◒"];

/// Which frame of the spinner a phase of a turn lands on.
///
/// Clamped rather than trusted: the caller's phase is `[0, 1)` by construction today, and this
/// indexes an array — a phase of exactly 1 from some later caller would be a panic in a
/// function whose whole job is to draw a character.
pub fn spinner(phase: f32) -> &'static str {
	// `as usize` saturates, so a negative phase and a `NaN` both land on the first frame.
	let frame = (phase.max(0.0) * SPINNER.len() as f32) as usize;
	SPINNER[frame.min(SPINNER.len() - 1)]
}

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

	/// The files pane's row pitch, which is what these cases are written in. The queues use
	/// 22 and the arithmetic does not care, which is the point of the parameter.
	const ROW: f32 = 24.0;

	#[test]
	fn a_turn_of_the_spinner_shows_every_frame_and_never_indexes_past_the_last() {
		// Arrange: one whole turn at the rate the sweep counter actually ticks.
		let turn: Vec<&str> = (0..30).map(|step| spinner(step as f32 / 30.0)).collect();

		// Act / Assert: all four frames appear, and in order — a spinner that skipped one
		// would still animate, which is why this counts them rather than eyeballing it.
		assert_eq!(turn[0], SPINNER[0]);
		let mut seen: Vec<&str> = Vec::new();
		for frame in turn {
			if seen.last() != Some(&frame) {
				seen.push(frame);
			}
		}
		assert_eq!(seen, SPINNER, "each frame once, in order");

		// And the values no caller sends today, because this indexes an array.
		assert_eq!(spinner(1.0), SPINNER[3], "a whole turn");
		assert_eq!(spinner(-1.0), SPINNER[0], "backwards");
		assert_eq!(spinner(f32::NAN), SPINNER[0], "not a number");
	}

	#[test]
	fn a_short_list_is_built_whole() {
		// Arrange / Act / Assert: the ordinary case — fewer rows than the cap, so nothing is
		// virtualized at all and the pane behaves exactly as it did before.
		let rows = |scroll, total| visible_rows(scroll, total, ROW, ROWS_BUILT);

		assert_eq!(rows(0.0, 0), 0..0, "an empty folder");
		assert_eq!(rows(0.0, 40), 0..40, "a music folder");
		assert_eq!(rows(0.0, ROWS_BUILT), 0..ROWS_BUILT, "exactly the cap");
	}

	#[test]
	fn scrolling_moves_the_window_by_whole_rows() {
		// Arrange: a list long enough that the window can move freely inside it.
		let total = 5_000;
		let rows = |scroll| visible_rows(scroll, total, ROW, ROWS_BUILT);

		// Act / Assert: the row under the top edge is the first one built, and it is built
		// even when only its bottom pixel shows — a partly visible row is a visible row.
		assert_eq!(rows(0.0).start, 0);
		assert_eq!(rows(ROW - 1.0).start, 0, "part way");
		assert_eq!(rows(ROW).start, 1, "exactly one down");
		assert_eq!(rows(100.0 * ROW).start, 100);

		// And the count is the cap wherever it sits.
		for row in [0, 1, 100, 4_000] {
			assert_eq!(rows(row as f32 * ROW).len(), ROWS_BUILT, "at row {row}");
		}
	}

	#[test]
	fn a_shorter_row_pitch_moves_the_window_sooner() {
		// Arrange / Act / Assert: a queue's rows are 22 pixels apart, not 24, and the same
		// offset therefore names a different row. The one thing the parameter buys, and the
		// one thing a shared helper could get wrong by using the wrong constant.
		assert_eq!(visible_rows(22.0, 5_000, 22.0, ROWS_BUILT).start, 1);
		assert_eq!(visible_rows(22.0, 5_000, 24.0, ROWS_BUILT).start, 0);
	}

	#[test]
	fn the_end_of_a_list_still_fills_the_pane() {
		// Arrange: scrolled to the very bottom, and then past it — which is what a stale
		// offset from a longer listing looks like.
		let total = 5_000;

		// Act / Assert: the window stops at the last full pane instead of running off the
		// end, so the bottom of a list is never a blank pane.
		let bottom = visible_rows(total as f32 * ROW, total, ROW, ROWS_BUILT);
		assert_eq!(bottom, (total - ROWS_BUILT)..total, "scrolled to the end");

		let past = visible_rows(1_000_000.0, total, ROW, ROWS_BUILT);
		assert_eq!(past, bottom, "an offset left over from a longer list");
	}

	#[test]
	fn an_impossible_offset_still_names_real_rows() {
		// Arrange / Act / Assert: `as usize` saturates rather than wrapping, which is what
		// this leans on — a negative or a `NaN` offset must land on the top of the list and
		// not on an index that would panic the `skip`/`take` below it.
		for scroll in [-1.0, -1_000_000.0, f32::NAN, f32::NEG_INFINITY] {
			let range = visible_rows(scroll, 5_000, ROW, ROWS_BUILT);
			assert_eq!(range.start, 0, "scroll {scroll}");
		}

		// The other end: an infinite offset is just a very stale one.
		assert_eq!(
			visible_rows(f32::INFINITY, 300, ROW, ROWS_BUILT),
			100..300,
			"an infinite offset"
		);
	}

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
