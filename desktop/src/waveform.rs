//! The waveform's arithmetic (PLAN §14a): folding a file's samples down to a small array
//! of amplitudes, and matching that array to the pixel columns drawn from it.
//!
//! Pure — no rodio, no iced, no filesystem. `audio::peaks` feeds `Fold` one decoded sample
//! at a time and `ui::waveform` reads the result back through `column_peak`, so the
//! arithmetic both ends depend on sits in one place and can be checked with no audio
//! device and no window (PLAN §12).

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
	fn a_scan_that_has_not_landed_yet_draws_as_silence() {
		// Arrange / Act / Assert: the state a player is in for the seconds a long track
		// takes to scan. Zero, not a panic and not a divide by nothing.
		assert_eq!(column_peak(&[], 0, 400), 0.0);
	}
}
