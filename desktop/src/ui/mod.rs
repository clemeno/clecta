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
pub mod tempo;
pub mod tree;
pub mod waveform;

use std::collections::BTreeMap;
use std::ops::Range;
use std::path::{Path, PathBuf};
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

/// How long a track's music runs, and how long the file is — for a row that has room for at
/// most one of them (PLAN §14c).
///
/// Two questions with three answers each, and the point of writing it once is that the files
/// pane and the queues give the same answer to both. `duration` carries its usual pair: `None`
/// is *not measured yet* and shows nothing at all, because a placeholder that flicks to a
/// number a moment later on every row of a freshly opened list is worse than a gap. `Some(None)`
/// is *measured, and there is no length* — a stream, or a file that will not open — and says so
/// with `--:--` rather than a zero it would be read as.
///
/// `music` is `None` until something has decoded the file, so a row shows its plain length
/// until it is prepared and `2:58 / 3:12` afterwards: the music first, because that is the
/// number a set is planned against, and the file's own length behind it, because that is the
/// one that has to match every other program on the machine.
pub fn format_lengths(music: Option<Duration>, duration: Option<Option<Duration>>) -> String {
	let whole = match duration {
		Some(Some(length)) => format_clock(length),
		Some(None) => "--:--".to_string(),
		None => String::new(),
	};

	match music {
		// A file scanned but not yet measured is the one order these can arrive in that would
		// otherwise print a trailing separator and nothing after it.
		Some(music) if whole.is_empty() => format_clock(music),
		Some(music) => format!("{} / {whole}", format_clock(music)),
		None => whole,
	}
}

/// A detected tempo, to two decimals (PLAN §14d).
///
/// The same two layers the lengths use, and the same three answers. Nothing at all until
/// something has scanned the file, because a column of placeholders that turn into numbers one by
/// one is worse than a column that fills in. `--` once it has been scanned and there was no tempo
/// to find — a spoken word recording, an ambient wash, a jingle too short to hold four beats —
/// which is an answer, and a different one from silence about it.
///
/// Two decimals always, including on a round 128: a column of numbers that sometimes has a
/// fractional part and sometimes does not is a column that has to be read rather than scanned,
/// and the width is the same either way.
///
/// No unit anywhere. Position says what it is, and three characters of "BPM" on every row of two
/// panes would cost more width than the number does.
pub fn format_tempo(tempo: Option<Option<f32>>) -> String {
	match tempo {
		Some(Some(tempo)) => format!("{tempo:.2}"),
		Some(None) => "--".to_string(),
		None => String::new(),
	}
}

/// The tempo a row actually shows: the one corrected by hand if there is one, and what the
/// detector said otherwise (PLAN §14d).
///
/// Applied **here, when the row is drawn**, rather than written into the model when the
/// correction is made. That is the difference between one rule and four: a correction would
/// otherwise have to be pushed into the files pane's map and into every queue holding the file,
/// and emptying the corrections would have to put back a detected number nothing kept a copy of.
/// This way the model holds what was measured, the file holds what was decided, and neither can
/// drift from the other.
///
/// A correction shows even on a file nothing has scanned — a folder cleared from the cache keeps
/// the number a person put there, which is exactly what makes it worth keeping in the other file.
pub fn edited_tempo(
	edits: &BTreeMap<PathBuf, f32>,
	path: &Path,
	detected: Option<Option<f32>>,
) -> Option<Option<f32>> {
	match edits.get(path) {
		Some(tempo) => Some(Some(*tempo)),
		None => detected,
	}
}

/// A selected row's background — the same fill in all three of the app's lists.
///
/// Here rather than in one of them because all three ask the same question and the answer has
/// to look the same: the files pane, the three queues, and the folder tree, where it says the
/// two panes agree about which folder is being shown. The queues borrow it for their scroll
/// edges too, which are lit while a drag hovers them for exactly the same reason — this is the
/// thing the pointer is on.
///
/// Every other row gets nothing at all, so the selection is the only thing the eye is drawn to.
pub fn row_style(theme: &iced::Theme, selected: bool) -> iced::widget::container::Style {
	use iced::widget::container;

	if !selected {
		return container::Style::default();
	}

	let palette = theme.extended_palette();
	container::Style {
		background: Some(palette.primary.weak.color.into()),
		text_color: Some(palette.primary.weak.text),
		..container::Style::default()
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
	fn a_row_says_nothing_until_it_knows_something_and_the_music_first_after() {
		// Arrange
		let secs = |seconds| Some(Duration::from_secs(seconds));

		// Act / Assert: an unmeasured row is blank, not a placeholder that flicks to a number a
		// moment later on every row of a freshly opened list.
		assert_eq!(format_lengths(None, None), "");

		// Measured, and the decoder had no length to give: said, not hidden, or the app would
		// re-open the file for ever waiting for the answer it already has.
		assert_eq!(format_lengths(None, Some(None)), "--:--");

		// Measured but never scanned, which is every row of a queue until a folder scan reaches
		// it: the file's own length and nothing else.
		assert_eq!(format_lengths(None, Some(secs(195))), "3:15");

		// Scanned: the music first, because that is the number a set is planned against.
		assert_eq!(format_lengths(secs(178), Some(secs(195))), "2:58 / 3:15");

		// Scanned before it was measured — the one order that would otherwise print a separator
		// with nothing after it.
		assert_eq!(format_lengths(secs(178), None), "2:58");
		assert_eq!(format_lengths(secs(178), Some(None)), "2:58 / --:--");
	}

	#[test]
	fn a_tempo_is_two_decimals_or_it_is_one_of_the_two_kinds_of_nothing() {
		// Act / Assert: the same three answers the lengths give, in the same order (PLAN §14d).
		assert_eq!(format_tempo(None), "", "nobody has scanned it");
		assert_eq!(
			format_tempo(Some(None)),
			"--",
			"scanned, and there is no tempo in it"
		);

		// Two decimals whatever the number, so the column is read down rather than across — and
		// rounded rather than truncated, since 127.999 is a 128 track.
		assert_eq!(format_tempo(Some(Some(128.0))), "128.00");
		assert_eq!(format_tempo(Some(Some(127.999))), "128.00");
		assert_eq!(format_tempo(Some(Some(97.4649))), "97.46");
		assert_eq!(
			format_tempo(Some(Some(174.005))),
			"174.01",
			"half a hundredth"
		);
	}

	#[test]
	fn a_corrected_tempo_wins_wherever_the_row_is_drawn() {
		// Arrange: one file corrected by hand, one left alone (PLAN §14d).
		let corrected = PathBuf::from("/music/half-time.mp3");
		let alone = PathBuf::from("/music/as-found.mp3");
		let edits = BTreeMap::from([(corrected.clone(), 87.0_f32)]);

		// Act / Assert: the correction replaces what the detector said, whatever that was —
		// including a file it found no tempo in at all, which would otherwise draw `--` for ever.
		assert_eq!(
			edited_tempo(&edits, &corrected, Some(Some(174.0))),
			Some(Some(87.0))
		);
		assert_eq!(
			edited_tempo(&edits, &corrected, Some(None)),
			Some(Some(87.0)),
			"scanned and found nothing"
		);

		// And it shows even on a file nothing has scanned — a folder dropped from the cache
		// keeps the number a person put there, which is why it lives in the other file.
		assert_eq!(edited_tempo(&edits, &corrected, None), Some(Some(87.0)));

		// A file nobody corrected is left exactly as it was found, in all three of its states.
		for detected in [None, Some(None), Some(Some(128.0))] {
			assert_eq!(edited_tempo(&edits, &alone, detected), detected);
		}
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
