//! The waveform's arithmetic (PLAN §14a, §14c): folding a file's samples down to a small
//! array of amplitudes, finding where the music inside the file starts and stops, and
//! matching the array to the pixel columns drawn from it.
//!
//! Pure — no rodio, no iced, no filesystem. `audio::scan` feeds `Fold` and `Edges` one
//! decoded sample at a time and `ui::waveform` reads the result back through `column_peak`,
//! so the arithmetic both ends depend on sits in one place and can be checked with no audio
//! device and no window (PLAN §12).

use std::time::Duration;

/// The most amplitudes a scan keeps. A finished scan holds between half this and this, so
/// the array is always at least as detailed as the widest panel anyone will drag the
/// window to and still small enough to clone into a `Message` without thinking about it.
const MAX_COLUMNS: usize = 2048;

/// A scan in progress: the running maximum of the samples seen so far, folded into a
/// bounded array whatever the length of the file.
///
/// The problem this solves is that the number of samples is **not known in advance** — a
/// stream has no duration at all (PLAN §7), so the samples-per-column cannot be worked out
/// up front. Instead every sample starts as its own column and the array is *halved* each
/// time it fills, doubling the samples each column stands for. One pass, no allocation
/// after the first `MAX_COLUMNS`, and the halvings together cost less than the final pass
/// over the array: each one touches half as many elements as the one before.
#[derive(Debug)]
pub struct Fold {
	columns: Vec<f32>,
	/// How many samples one finished column stands for. Doubles at every halving.
	per_column: u32,
	/// The loudest sample in the column being built, and how many are in it so far.
	current: f32,
	filled: u32,
}

impl Default for Fold {
	fn default() -> Self {
		Self {
			columns: Vec::new(),
			// One sample per column to begin with: the halving below is what coarsens it,
			// and starting at zero would divide by nothing.
			per_column: 1,
			current: 0.0,
			filled: 0,
		}
	}
}

impl Fold {
	/// Add one decoded sample.
	///
	/// Channels are not separated: a stereo file folds both into the same column, which is
	/// what an amplitude envelope wants and what makes this independent of the channel
	/// count the decoder happens to report.
	///
	/// `f32::max` returns the other operand when one is `NaN`, so a decoder that emits one
	/// cannot poison the whole column — which is the behaviour we want and the reason the
	/// comparison is written this way round.
	pub fn push(&mut self, sample: f32) {
		self.current = self.current.max(sample.abs());
		self.filled += 1;

		if self.filled == self.per_column {
			self.columns.push(self.current);
			self.current = 0.0;
			self.filled = 0;

			if self.columns.len() == MAX_COLUMNS {
				self.halve();
			}
		}
	}

	/// Fold pairs of columns into one, halving the array and doubling what a column means.
	fn halve(&mut self) {
		for index in 0..MAX_COLUMNS / 2 {
			self.columns[index] = self.columns[2 * index].max(self.columns[2 * index + 1]);
		}
		self.columns.truncate(MAX_COLUMNS / 2);
		self.per_column *= 2;
	}

	/// The finished scan.
	///
	/// The partly-filled last column is kept rather than dropped: on a short file it is a
	/// real part of the sound, and on a long one it is the end of the track, which is
	/// exactly where an eye goes.
	pub fn finish(mut self) -> Vec<f32> {
		if self.filled > 0 {
			self.columns.push(self.current);
		}
		self.columns
	}
}

/// The quietest sample that still counts as music, as a fraction of full scale.
///
/// −50 dBFS. Digital silence is 0 and a mastered track sits within a few dB of 1, so
/// anything in between is a judgement — and this is the knob for it. Low enough that a fade
/// is not clipped and a quiet intro is not mistaken for the leader; high enough to sit above
/// the dither and the tape hiss a rip carries, which are the two things that would otherwise
/// make every file's music start at sample zero.
///
/// `ponytail:` one threshold for every file, rather than one derived from the track's own
/// noise floor. A vinyl rip with a loud floor reads as music throughout and simply gets no
/// trim, which is the same answer as not having scanned it. The upgrade is a percentile of
/// the file's own amplitudes, which needs the whole array kept rather than a running pair.
const SILENCE: f32 = 0.003_162_3;

/// Where the music actually sits inside a file (PLAN §14c).
///
/// Both ends are measured from the start of the file, so `start` is what to seek to and
/// `end` is when the track is over as far as anybody listening is concerned. A file with no
/// leader and no run-out gives `0` and its full length, which is why there is no separate
/// "this file needs no trimming" state — the numbers say it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Trim {
	pub start: Duration,
	pub end: Duration,
}

impl Trim {
	/// How long the music itself runs: the file's length with the leader and the run-out taken
	/// off, which is what a set is actually made of (PLAN §14c).
	///
	/// Derived, never stored. The two edges are already in the cache and this is arithmetic on
	/// them — a third number written beside them would be a third number that can disagree with
	/// them, and the one that is wrong would be the one on screen.
	///
	/// `saturating_sub` for a subtraction that cannot underflow today: `end` is one past the
	/// last loud sample and `start` is the first, so a `Trim` that exists has `end > start`. It
	/// is here because this draws a column, and a panic in a column is not worth two characters.
	pub fn music(self) -> Duration {
		self.end.saturating_sub(self.start)
	}
}

/// The two edges of the music, found in the same pass that folds the waveform.
///
/// Sample-exact rather than read off the finished peak array, which is the whole reason this
/// is a second accumulator instead of two lines in `column_peak`'s caller: a scan holds at
/// most 2048 columns however long the file, so one column of a five-minute track is a sixth
/// of a second. Trimming to a *column* would clip the first transient or leave a sixth of a
/// second of leader — audible either way, and this costs one comparison per sample.
#[derive(Debug, Default)]
pub struct Edges {
	/// How many samples have been pushed, which is also the index of the next one.
	seen: u64,
	/// The first sample above the threshold, and one past the last.
	first: Option<u64>,
	last: u64,
}

impl Edges {
	/// Add one decoded sample, the same one `Fold::push` is given.
	///
	/// `>` and not `>=`, so a file of digital silence has no edges at all rather than edges
	/// at its two ends. A `NaN` fails the comparison and therefore reads as silence, which is
	/// the same answer `Fold` gives it.
	pub fn push(&mut self, sample: f32) {
		self.seen += 1;

		if sample.abs() > SILENCE {
			self.first.get_or_insert(self.seen - 1);
			self.last = self.seen;
		}
	}

	/// The edges as times, or `None` for a file with no music in it at all — which is a real
	/// answer worth storing rather than a failure: it is what stops the app trimming a track
	/// of pure silence down to nothing.
	///
	/// `rate` and `channels` are the stream's, and the product is how many samples a second
	/// of the file holds: the decoder interleaves channels, so a stereo second is twice the
	/// sample rate. Zero for either is impossible from rodio (both are `NonZero`) and is
	/// guarded anyway, because this divides by it.
	pub fn finish(self, rate: u32, channels: u16) -> Option<Trim> {
		let per_second = f64::from(rate) * f64::from(channels);
		let first = self.first?;
		if per_second <= 0.0 {
			return None;
		}

		Some(Trim {
			start: Duration::from_secs_f64(first as f64 / per_second),
			end: Duration::from_secs_f64(self.last as f64 / per_second),
		})
	}
}

/// The amplitude to draw at one of `columns` pixel columns.
///
/// A scan's length has nothing to do with the width of the panel it is drawn in, so the
/// two are matched here rather than at either end: a wide widget repeats a scan column
/// across several pixels, a narrow one takes the **maximum** over the range it covers.
///
/// Maximum, not average. Averaging a waveform down to a few hundred pixels flattens
/// precisely the transients the display exists to show — a kick drum becomes a bump.
pub fn column_peak(peaks: &[f32], column: usize, columns: usize) -> f32 {
	if peaks.is_empty() || columns == 0 {
		return 0.0;
	}

	// Clamped rather than trusted: `columns` comes from a widget's measured width and
	// `column` from a loop over it, and a slice out of range is a panic in a draw call.
	let start = (column * peaks.len() / columns).min(peaks.len() - 1);
	let end = ((column + 1) * peaks.len() / columns)
		.max(start + 1)
		.min(peaks.len());

	peaks[start..end].iter().copied().fold(0.0, f32::max)
}

/// How much of the strip the scanning band covers, as a fraction of its width.
const SWEEP_WIDTH: f32 = 0.18;

/// The visible part of the scanning band, as `(start, end)` offsets into a strip `width`
/// wide, or `None` when the band is entirely off one end.
///
/// `phase` runs `0.0..=1.0` and the band travels from *fully off the left* to *fully off
/// the right*, so it slides in and out rather than appearing and vanishing at the edges.
/// That is the whole reason this is arithmetic worth its own function: the clamping at both
/// ends is where an off-by-one would draw a band hanging outside the strip, and the caller
/// is a `draw`.
pub fn sweep_band(width: f32, phase: f32) -> Option<(f32, f32)> {
	if width <= 0.0 {
		return None;
	}

	let band = width * SWEEP_WIDTH;
	let left = phase.clamp(0.0, 1.0) * (width + band) - band;

	let start = left.max(0.0);
	let end = (left + band).min(width);

	(end > start).then_some((start, end))
}

/// Where a click `x` pixels into a strip `width` wide lands in the track, as a fraction of
/// its length. `None` for a strip with no width.
///
/// The `None` is not politeness about a degenerate case. The caller multiplies a `Duration`
/// by this, and `Duration::mul_f32` **panics** on a `NaN` rather than saturating — so a
/// mis-measured strip would take the app down mid-click.
///
/// Guarding the width alone is not enough, which the test below found rather than the
/// author: `f32::clamp` passes a `NaN` **straight through**, so `clamp(0.0, 1.0)` is not a
/// range guarantee at all. The range test on the way out is the actual guard; the clamp
/// only handles a pointer dragged past either edge.
pub fn seek_fraction(width: f32, x: f32) -> Option<f32> {
	if width <= 0.0 {
		return None;
	}

	let fraction = (x / width).clamp(0.0, 1.0);
	(0.0..=1.0).contains(&fraction).then_some(fraction)
}

#[cfg(test)]
mod tests {
	use super::*;

	/// Fold a whole slice, the way `audio::peaks` folds a whole file.
	fn scan(samples: &[f32]) -> Vec<f32> {
		let mut fold = Fold::default();
		for sample in samples {
			fold.push(*sample);
		}
		fold.finish()
	}

	#[test]
	fn a_short_file_keeps_every_sample_it_has() {
		// Arrange / Act / Assert: below `MAX_COLUMNS` nothing is folded at all, and the
		// sign is dropped because a waveform is an envelope.
		assert_eq!(scan(&[0.5, -1.0, 0.25]), vec![0.5, 1.0, 0.25]);
	}

	#[test]
	fn a_scan_stays_bounded_however_long_the_file_is() {
		// Arrange: a million samples is a few seconds of audio, and already 500× more than
		// the array is allowed to hold.
		let samples: Vec<f32> = (0..1_000_000).map(|n| (n % 100) as f32 / 100.0).collect();

		// Act
		let peaks = scan(&samples);

		// Assert: between a half-full and a full array, whatever the input length — this is
		// the whole point of the halving, and what keeps a `Message` cheap to clone.
		assert!(
			(MAX_COLUMNS / 2..=MAX_COLUMNS).contains(&peaks.len()),
			"{} columns",
			peaks.len()
		);
	}

	#[test]
	fn folding_never_loses_the_loudest_sample() {
		// Arrange: quiet everywhere but one spike, placed late so it survives several
		// halvings rather than none.
		let mut samples = vec![0.1_f32; 500_000];
		samples[400_000] = 0.93;

		// Act
		let peaks = scan(&samples);

		// Assert: the maximum, not an average, is what a halving keeps.
		let loudest = peaks.iter().copied().fold(0.0, f32::max);
		assert_eq!(loudest, 0.93);
	}

	#[test]
	fn a_not_a_number_sample_does_not_poison_its_column() {
		// Arrange / Act: NaN cannot arrive from JSON but can from arithmetic in a decoder,
		// and one bad sample must not blank a column.
		let peaks = scan(&[0.4, f32::NAN, 0.2]);

		// Assert
		assert_eq!(peaks, vec![0.4, 0.0, 0.2]);
	}

	/// Push a whole slice through `Edges`, the way `audio::scan` pushes a whole file.
	fn edges(samples: &[f32], rate: u32, channels: u16) -> Option<Trim> {
		let mut edges = Edges::default();
		for sample in samples {
			edges.push(*sample);
		}
		edges.finish(rate, channels)
	}

	#[test]
	fn the_music_starts_after_the_leader_and_stops_before_the_run_out() {
		// Arrange: a tenth of a second of digital silence, two tenths of music, a tenth of
		// silence again — at a rate that makes the answer readable.
		let mut samples = vec![0.0_f32; 400];
		samples[100..300].fill(0.8);

		// Act
		let trim = edges(&samples, 1_000, 1).expect("a file with music in it");

		// Assert: exactly, because the whole reason this is measured per sample rather than
		// per waveform column is that a sixth of a second either way is audible (PLAN §14c).
		assert_eq!(trim.start, Duration::from_millis(100));
		assert_eq!(trim.end, Duration::from_millis(300));
	}

	#[test]
	fn a_channel_is_not_a_second() {
		// Arrange / Act: the same samples read as stereo, where the decoder interleaves two
		// channels into every frame — so the file is half as long as its sample count says.
		let mut samples = vec![0.0_f32; 400];
		samples[100..300].fill(0.8);

		// Assert: getting this wrong is a trim that lands twice as far in as the music does,
		// which on a handover means the next track starts halfway through its first verse.
		let trim = edges(&samples, 1_000, 2).expect("a file with music in it");
		assert_eq!(trim.start, Duration::from_millis(50));
		assert_eq!(trim.end, Duration::from_millis(150));
	}

	#[test]
	fn the_playing_time_is_what_is_left_between_the_edges() {
		// Arrange: a track with four seconds of leader and two of run-out.
		let mut samples = vec![0.0_f32; 12_000];
		samples[4_000..10_000].fill(0.8);

		// Act / Assert: six seconds of music in a twelve-second file, and it is arithmetic on
		// the two numbers already in the store rather than a third one written beside them.
		let trim = edges(&samples, 1_000, 1).expect("a file with music in it");
		assert_eq!(trim.music(), Duration::from_secs(6));

		// Edges the wrong way round cannot happen — `end` is one past the last loud sample —
		// but this draws a column, so it gives a `0:00` rather than a panic if they ever are.
		assert_eq!(
			Trim {
				start: Duration::from_secs(9),
				end: Duration::from_secs(1),
			}
			.music(),
			Duration::ZERO
		);
	}

	#[test]
	fn a_file_with_no_music_in_it_has_no_edges() {
		// Arrange / Act / Assert: silence, and near-silence under the threshold. `None` is an
		// answer rather than a failure — it is what stops a track of pure silence being
		// trimmed away to nothing.
		assert_eq!(edges(&[0.0; 100], 1_000, 1), None, "digital silence");
		assert_eq!(edges(&[0.0005; 100], 1_000, 1), None, "under the threshold");
		assert_eq!(edges(&[], 1_000, 1), None, "no samples at all");

		// And a `NaN` is silence, the same answer `Fold` gives it: a decoder that emits one
		// must not make a file's music appear to start there.
		assert_eq!(edges(&[f32::NAN; 100], 1_000, 1), None, "not a number");
	}

	#[test]
	fn one_loud_sample_is_enough_to_be_the_edge() {
		// Arrange / Act: a click in the leader, which is what a rip of a scratched record
		// puts there.
		let mut samples = vec![0.0_f32; 400];
		samples[10] = 0.9;
		samples[200] = 0.9;

		// Assert: the click wins, and it is the ceiling named on `SILENCE` — no hold time, so
		// a lone tick before the music reads as the start of it. A file that trims wrongly is
		// a file to re-scan after the threshold moves, which is what the button is for.
		let trim = edges(&samples, 1_000, 1).expect("a file with a click in it");
		assert_eq!(trim.start, Duration::from_millis(10));
	}

	#[test]
	fn a_narrow_widget_takes_the_peak_of_the_range_it_covers() {
		// Arrange: four scan columns drawn in two pixels.
		let peaks = [0.1, 0.8, 0.3, 0.2];

		// Act / Assert: the loud one survives being squeezed, which averaging would lose.
		assert_eq!(column_peak(&peaks, 0, 2), 0.8);
		assert_eq!(column_peak(&peaks, 1, 2), 0.3);
	}

	#[test]
	fn a_wide_widget_repeats_a_scan_column_across_pixels() {
		// Arrange: two scan columns drawn in four pixels.
		let peaks = [0.25, 0.75];

		// Act / Assert
		let drawn: Vec<f32> = (0..4)
			.map(|column| column_peak(&peaks, column, 4))
			.collect();
		assert_eq!(drawn, vec![0.25, 0.25, 0.75, 0.75]);
	}

	#[test]
	fn every_column_of_every_width_is_in_range() {
		// Arrange: the guard that matters, because the caller is a `draw` and a slice out
		// of range there is a panic mid-frame.
		let peaks = [0.1, 0.2, 0.3, 0.4, 0.5];

		// Act / Assert: including a widget wider and narrower than the scan, and a column
		// past the end of its own width.
		for columns in [0, 1, 3, 5, 9, 400] {
			for column in 0..=columns + 1 {
				let value = column_peak(&peaks, column, columns);
				assert!(
					(0.0..=0.5).contains(&value),
					"{column} of {columns} gave {value}"
				);
			}
		}
	}

	#[test]
	fn the_scanning_band_slides_in_and_out_rather_than_appearing() {
		// Arrange / Act / Assert: off both ends at the extremes, and fully inside halfway.
		assert_eq!(sweep_band(100.0, 0.0), None, "still off the left");
		assert_eq!(sweep_band(100.0, 1.0), None, "already off the right");

		let (start, end) = sweep_band(100.0, 0.5).expect("visible halfway");
		assert!(
			start > 0.0 && end < 100.0,
			"{start}..{end} should be inside"
		);
		assert!((end - start - 18.0).abs() < 0.01, "full width halfway");
	}

	#[test]
	fn the_scanning_band_never_leaves_the_strip() {
		// Arrange / Act / Assert: the guard, for the same reason as the columns above —
		// this is read by a `draw`, including on a strip of no width at all.
		assert_eq!(sweep_band(0.0, 0.5), None, "a strip with no width");

		for step in 0..=100 {
			let phase = step as f32 / 100.0;
			if let Some((start, end)) = sweep_band(240.0, phase) {
				assert!(
					start >= 0.0 && end <= 240.0 && start < end,
					"phase {phase} gave {start}..{end}"
				);
			}
		}
	}

	#[test]
	fn a_click_maps_to_the_fraction_of_the_track_under_it() {
		// Arrange / Act / Assert: both ends exact, because clicking the very start of a
		// strip has to mean 0 and not "nearly 0".
		assert_eq!(seek_fraction(400.0, 0.0), Some(0.0));
		assert_eq!(seek_fraction(400.0, 400.0), Some(1.0));
		assert_eq!(seek_fraction(400.0, 100.0), Some(0.25));
	}

	#[test]
	fn a_click_can_never_produce_a_fraction_that_panics_a_duration() {
		// Arrange / Act / Assert: `Duration::mul_f32` panics on a `NaN` or a negative, so
		// this is the guard that keeps a mis-measured strip from taking the app down.
		assert_eq!(seek_fraction(0.0, 10.0), None, "a strip with no width");
		assert_eq!(seek_fraction(-5.0, 10.0), None, "a negative width");

		// A pointer past either edge — reachable while a button is held and dragged.
		assert_eq!(seek_fraction(400.0, -30.0), Some(0.0), "left of the strip");
		assert_eq!(seek_fraction(400.0, 900.0), Some(1.0), "right of the strip");

		for (width, x) in [(400.0, f32::NAN), (f32::NAN, 10.0), (400.0, f32::INFINITY)] {
			let fraction = seek_fraction(width, x);
			assert!(
				fraction.is_none_or(|f| (0.0..=1.0).contains(&f)),
				"width {width}, x {x} gave {fraction:?}"
			);
		}
	}

	#[test]
	fn a_scan_that_has_not_landed_yet_draws_as_silence() {
		// Arrange / Act / Assert: the state a player is in for the seconds a long track
		// takes to scan. Zero, not a panic and not a divide by nothing.
		assert_eq!(column_peak(&[], 0, 400), 0.0);
	}
}
