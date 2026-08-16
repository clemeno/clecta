//! What is in a file (PLAN §14a, §14c, §14d): its samples folded down to a small array of
//! amplitudes, where the music inside it starts and stops, and how fast it beats.
//!
//! Pure — no rodio, no iced, no filesystem. `audio::scan` opens the file and hands every sample
//! to a `Scanner`; everything after that is arithmetic, which is why all of it can be checked
//! with no audio device and no window (PLAN §12).
//!
//! The interface is small on purpose: `Scanner` and the `Scan` it produces, `Trim` and the one
//! question anybody asks it, and `column_peak` for reading a finished array back at whatever
//! width a strip happens to be. The three accumulators behind `Scanner` are private — they are
//! how it works, not what it offers, and their tests reach them directly from inside this module
//! because a seam inside a module is still a seam.
//!
//! What is **not** here any more is the strip's own geometry (Q46): `sweep_band` and
//! `seek_fraction` take a width in pixels and never ask this module anything, so they live with
//! the widget that draws them.

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
struct Fold {
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

/// What one decode of a whole file works out about it (PLAN §14a, §14c, §14d).
///
/// Three answers from one pass, because the pass is the expensive part: the array the waveform
/// draws, where the music inside the file starts and stops, and how fast it beats. Splitting them
/// into three functions would mean decoding every track three times for facts that arrive
/// together.
///
/// Declared here rather than in `audio`, which is the module that produces it, so that `cache`
/// can name it without depending on the one module that knows rodio exists (PLAN §4). Nothing
/// in it is about playback: it is three numbers about a file, and the three accumulators that
/// find them are its neighbours.
///
/// `Clone` because it travels inside a `Message` (PLAN §5), which iced requires to be one —
/// eight kilobytes of amplitudes copied once per track loaded. `PartialEq` so a test can say
/// "what came back out of the store is what went in" in one line rather than three.
#[derive(Debug, Clone, PartialEq)]
pub struct Scan {
	pub peaks: Vec<f32>,
	/// `None` for a file with nothing above the silence threshold in it (PLAN §14c).
	pub trim: Option<Trim>,
	/// Beats per minute, and `None` for a file with no tempo to find: silence, speech, or a
	/// recording too short to hold a few beats (PLAN §14d).
	pub tempo: Option<f32>,
}

/// The two edges of the music, found in the same pass that folds the waveform.
///
/// Sample-exact rather than read off the finished peak array, which is the whole reason this
/// is a second accumulator instead of two lines in `column_peak`'s caller: a scan holds at
/// most 2048 columns however long the file, so one column of a five-minute track is a sixth
/// of a second. Trimming to a *column* would clip the first transient or leave a sixth of a
/// second of leader — audible either way, and this costs one comparison per sample.
#[derive(Debug, Default)]
struct Edges {
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

/// How many interleaved samples one envelope bin covers (PLAN §14d).
///
/// A power of two for no reason other than habit; what matters is the *time* it works out to,
/// which is 11.6 ms of a mono 44.1 kHz file and 5.8 ms of a stereo one. Both are short enough
/// to put a kick drum in a bin of its own — a beat has to be locatable to a few milliseconds or
/// the tempo it implies is wrong in the first decimal — and long enough that a five-minute
/// track is a few tens of thousands of bins rather than millions.
const TEMPO_BIN: u32 = 512;

/// The tempi the search is allowed to answer with, in beats per minute.
///
/// Deliberately *not* folded into a narrower window (PLAN §14d): a detector that quietly doubles
/// a 70 and halves a 174 is one that cannot be argued with, and the answer to an octave that
/// lands wrong is a person looking at it, not a constant. The range is the range of tempi people
/// actually mix, and nothing outside it is worth reporting.
const MIN_BPM: f32 = 65.0;
const MAX_BPM: f32 = 200.0;

/// Added before taking a logarithm, so a bin of digital silence is a small number rather than
/// negative infinity — which would make every difference either side of it meaningless.
const QUIET: f32 = 1e-6;

/// A tempo being worked out, in the same pass that folds the waveform and finds the edges
/// (PLAN §14d).
///
/// The third accumulator on one decode, and the reason it is a third rather than a second pass
/// is the same reason `Edges` is: the file is already being read a sample at a time, and the only
/// expensive thing about any of this is the decoding.
///
/// What it keeps is a *loudness envelope* — one number per `TEMPO_BIN` samples — because a tempo
/// is not in the samples themselves but in when they get suddenly louder. Everything else happens
/// in `finish`, on an array a thousandth the size of the file.
///
/// `ponytail:` the envelope is unbounded — one `f32` per 512 samples, so 400 KB for a ten-minute
/// stereo track, held only while the scan runs. `Fold` halves itself instead, which this cannot
/// do: halving would coarsen the time resolution the whole answer rests on. Cap the *analysed*
/// length instead if a two-hour recording ever needs to be scanned.
#[derive(Debug, Default)]
struct Tempo {
	bins: Vec<f32>,
	/// The bin being filled: the sum of the sample magnitudes in it, and how many there are.
	current: f32,
	filled: u32,
}

impl Tempo {
	/// Add one decoded sample, the same one `Fold` and `Edges` are given.
	///
	/// Channels are summed together like the waveform's, because a beat is in both of them and
	/// the sum is the loudest thing about it.
	///
	/// A `NaN` is skipped rather than added: it would poison its bin, and then the two
	/// differences either side of that bin, and a decoder that emits one must not be able to
	/// invent an onset.
	pub fn push(&mut self, sample: f32) {
		if sample.is_finite() {
			self.current += sample.abs();
		}
		self.filled += 1;

		if self.filled == TEMPO_BIN {
			self.bins.push(self.current);
			self.current = 0.0;
			self.filled = 0;
		}
	}

	/// The tempo in beats per minute, or `None` when the file has none to give: silence, a
	/// recording too short to hold a few beats, or a stream whose rate could not be read.
	///
	/// The partly-filled last bin is **dropped**, unlike the waveform's, and the difference is
	/// what the two arrays are for: a bin holding half as many samples is quieter through no
	/// fault of the music, and this array is read as a series of loudness *changes*.
	pub fn finish(self, rate: u32, channels: u16) -> Option<f32> {
		let per_bin = f64::from(TEMPO_BIN) / (f64::from(rate) * f64::from(channels));
		if per_bin <= 0.0 {
			return None;
		}

		let onsets = onsets(&self.bins);
		// The slowest tempo asked about, in bins, which is also the shortest useful recording:
		// a comparison with nothing like two of the longest beats to run over says nothing.
		let longest = (60.0 / (f64::from(MIN_BPM) * per_bin)).round() as usize;
		let shortest = (60.0 / (f64::from(MAX_BPM) * per_bin)).round() as usize;
		// Two bins is the floor, not one: the refining pass sweeps a bin either side of what this
		// finds, and a period it could walk down to zero is a division by nothing. Reachable only
		// from a stream of a couple of kilohertz, which is why it is a guard and not a feature.
		if shortest < 2 || onsets.len() < 2 * longest {
			return None;
		}

		// The coarse pass: whole bins, and the first of any equal pair wins, which means the
		// faster of two tempi that fit the same beats equally well.
		let mut best = (f32::NEG_INFINITY, shortest);
		for lag in shortest..=longest {
			let score = agreement(&onsets, lag);
			if score > best.0 {
				best = (score, lag);
			}
		}
		if best.0 <= 0.0 {
			return None;
		}

		let period = refine(&onsets, best.1 as f32);
		let bpm = 60.0 / (f64::from(period) * per_bin);

		// The store keeps whatever comes out of here and the column prints it, so the one number
		// that must not leave this function is one that could be drawn as a tempo and is not.
		(bpm.is_finite() && bpm > 0.0).then_some(bpm as f32)
	}
}

/// One decode's worth of accumulation: push every sample in, get a `Scan` out (Q46).
///
/// The three accumulators above are identical in shape — `Default`, `push`, `finish` — and were
/// never independent: they are three answers to the one expensive question, which is "what is in
/// this file", and the expensive part is the decode they share. Driving them separately meant the
/// caller had to know there were three of them, that two of the three need the sample rate and
/// the channel count and one does not, and that all three must be fed *every* sample or their
/// answers stop meaning anything about the same audio.
///
/// So the drive loop lives here, once, instead of in `audio::scan` and again in each of the three
/// test modules. The three stay separate types behind it, and their tests still reach them one at
/// a time — a seam inside a module is still a seam; it is just not part of what the module
/// promises.
///
/// Adding a fourth fact is now a struct, a field, a `push` and a `finish` — all of them in this
/// file, and none of them anywhere else.
#[derive(Debug, Default)]
pub struct Scanner {
	fold: Fold,
	edges: Edges,
	tempo: Tempo,
}

impl Scanner {
	/// One decoded sample, to all three.
	///
	/// Every sample, in order, with nothing skipped: `Fold` counts them to know how wide a
	/// column is, `Edges` counts them to know where the music starts, and `Tempo` counts them to
	/// know how long a bin is. A sample given to two of the three would put the third's answer
	/// out by however many it missed.
	pub fn push(&mut self, sample: f32) {
		self.fold.push(sample);
		self.edges.push(sample);
		self.tempo.push(sample);
	}

	/// The three answers, given the format the samples arrived in.
	///
	/// The rate and the channel count are asked for here rather than at the start because only
	/// the two accumulators that report *times* need them — and asking once, at the end, is what
	/// keeps `push` a function of one argument.
	pub fn finish(self, rate: u32, channels: u16) -> Scan {
		Scan {
			peaks: self.fold.finish(),
			trim: self.edges.finish(rate, channels),
			tempo: self.tempo.finish(rate, channels),
		}
	}
}

/// The loudness envelope turned into a track of *onsets*: how much louder each bin is than the
/// one before it, and nothing at all when it is quieter.
///
/// Three things happen here and each earns its line. The logarithm is what makes a beat in a
/// quiet passage weigh the same as one in a loud passage — an ear hears ratios, and an amplitude
/// difference would let the loudest thirty seconds of a track decide its tempo. The `max(0.0)`
/// keeps only the rises, because a note starting is a beat and a note ending is not. And the mean
/// is taken back off so that what is left is a train of spikes around zero: without that, every
/// comparison below would be dominated by the constant the spikes sit on rather than by where
/// they are.
fn onsets(bins: &[f32]) -> Vec<f32> {
	let mut rises: Vec<f32> = bins
		.windows(2)
		.map(|pair| ((pair[1] + QUIET).ln() - (pair[0] + QUIET).ln()).max(0.0))
		.collect();

	if rises.is_empty() {
		return rises;
	}

	let mean = rises.iter().sum::<f32>() / rises.len() as f32;
	for rise in &mut rises {
		*rise -= mean;
	}
	rises
}

/// How well the onset track agrees with itself `lag` bins later — the number the coarse pass
/// maximises, and the sense in which a file "has" a tempo at all.
///
/// Deliberately **not** divided by the number of terms in the sum. The shorter the lag the more
/// terms there are, so a lag and its double are not weighed equally — the double is worth
/// slightly less, and that is what settles a tie in favour of the faster tempo rather than
/// leaving it to the last bit of a float.
fn agreement(onsets: &[f32], lag: usize) -> f32 {
	if lag == 0 || lag >= onsets.len() {
		return 0.0;
	}

	let sum: f32 = onsets
		.iter()
		.zip(&onsets[lag..])
		.map(|(early, late)| early * late)
		.sum();

	sum / onsets.len() as f32
}

/// How much of the onset track beats at exactly this period — one bin of a Fourier transform,
/// evaluated at one frequency instead of all of them.
///
/// This is what the second decimal is made of, and the reason the refining pass is not simply
/// `agreement` at a fractional lag. A lag that falls between two bins has to be read by
/// interpolating them, and a straight line between two samples has its maximum at one end or the
/// other — so a correlation refined that way quietly snaps back to whole bins, which at 128 BPM is
/// three BPM wide. A rotating phasor has no such steps: it is smooth in the period, so the peak
/// can sit anywhere between two bins, and every beat in the track pulls on where it sits.
///
/// The phasor is turned by one complex multiplication per bin rather than a `cos` and a `sin`,
/// which is what makes a hundred candidates over a ten-minute track affordable. `f64` for the
/// turning, because a hundred thousand multiplications of a unit vector by itself is exactly the
/// place where `f32` drifts off the unit circle.
fn strength(onsets: &[f32], period: f32) -> f32 {
	if period <= 1.0 || onsets.is_empty() {
		return 0.0;
	}

	let angle = -std::f64::consts::TAU / f64::from(period);
	let (turn_cos, turn_sin) = (angle.cos(), angle.sin());
	let (mut phase_cos, mut phase_sin) = (1.0_f64, 0.0_f64);
	let (mut real, mut imaginary) = (0.0_f64, 0.0_f64);

	for onset in onsets {
		let onset = f64::from(*onset);
		real += onset * phase_cos;
		imaginary += onset * phase_sin;

		let turned = phase_cos * turn_cos - phase_sin * turn_sin;
		phase_sin = phase_cos * turn_sin + phase_sin * turn_cos;
		phase_cos = turned;
	}

	(real.hypot(imaginary) / onsets.len() as f64) as f32
}

/// The period to a thousandth of a bin, starting from the whole bin the coarse pass found.
///
/// Two narrowing passes rather than one fine sweep: there is a single peak within a bin either
/// side of the coarse answer, so sixty-five candidates twice reach a step of a thousandth of a bin
/// where one pass at that step would be two thousand candidates — and each of them costs a run
/// over the whole onset track. A thousandth of a bin is a fortieth of a BPM at 128, which is the
/// precision the column claims and a little better.
fn refine(onsets: &[f32], coarse: f32) -> f32 {
	let mut period = coarse;
	let mut span = 1.0;

	for _ in 0..2 {
		let step = span / 32.0;
		let mut best = (f32::NEG_INFINITY, period);

		for offset in -32..=32 {
			let candidate = period + offset as f32 * step;
			let score = strength(onsets, candidate);
			if score > best.0 {
				best = (score, candidate);
			}
		}

		period = best.1;
		span = step;
	}

	period
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

	/// A click track at a given tempo: a ten-millisecond decaying tick on every beat and digital
	/// silence between them. The beats are placed at the *rounded* multiples of a fractional
	/// period, so the average tempo really is the one asked for rather than the one an integer
	/// number of samples per beat would round it to.
	fn clicks(bpm: f32, seconds: f32, rate: u32) -> Vec<f32> {
		let total = (seconds * rate as f32) as usize;
		let period = 60.0 / bpm * rate as f32;
		let tick = rate as usize / 100;
		let mut samples = vec![0.0_f32; total];

		for beat in 0.. {
			let at = (beat as f32 * period).round() as usize;
			if at >= total {
				break;
			}
			for step in 0..tick {
				if let Some(sample) = samples.get_mut(at + step) {
					*sample = 0.8 * (1.0 - step as f32 / tick as f32);
				}
			}
		}

		samples
	}

	/// Push a whole slice through `Tempo`, the way `audio::scan` pushes a whole file.
	fn tempo(samples: &[f32], rate: u32, channels: u16) -> Option<f32> {
		let mut tempo = Tempo::default();
		for sample in samples {
			tempo.push(*sample);
		}
		tempo.finish(rate, channels)
	}

	#[test]
	fn a_click_track_reads_back_the_tempo_it_was_built_at() {
		// Arrange / Act / Assert: three tempi across the range, each to a fiftieth of a BPM —
		// which is what makes the second decimal worth printing (PLAN §14d). A bin is 11.6 ms
		// here, so a beat is 43 of them at 128 and this is a *two-thousandth* of a bin: the
		// precision comes from the phasor turning over the whole track, not from the bins.
		for asked in [100.0, 128.0, 174.0] {
			let found = tempo(&clicks(asked, 20.0, 44_100), 44_100, 1)
				.unwrap_or_else(|| panic!("{asked} BPM of clicks has a tempo"));
			assert!(
				(found - asked).abs() < 0.02,
				"{asked} BPM read back as {found}"
			);
		}

		// And 174 is the interesting one: 87 is inside the range too and fits every other click
		// exactly as well. The faster reading wins because the comparison is not divided by the
		// number of beats it had to work with (PLAN §14d) — the answer the manual editor then
		// exists to overrule.
	}

	#[test]
	fn a_channel_is_not_a_beat_either() {
		// Arrange: the same click track read as stereo, where every frame is two samples — so
		// the file is half as long and the beats are twice as often as the count alone says.
		let samples = clicks(128.0, 20.0, 44_100);

		// Act / Assert: getting this wrong is a tempo out by a factor of two, which is the one
		// mistake a detector is not allowed to make quietly.
		let found = tempo(&samples, 22_050, 2).expect("a click track has a tempo");
		assert!((found - 128.0).abs() < 0.1, "read back as {found}");
	}

	#[test]
	fn a_file_with_nothing_to_count_has_no_tempo() {
		// Arrange / Act / Assert: silence has no onsets at all, and a `None` here is what puts a
		// `--` in the column rather than a number somebody would act on.
		assert_eq!(tempo(&[0.0; 441_000], 44_100, 1), None, "digital silence");

		// Too short to hold two of the slowest beats it is asked about: under two seconds, where
		// 65 BPM is nearly one beat a second. An answer from that would be a guess.
		assert_eq!(
			tempo(&clicks(128.0, 1.0, 44_100), 44_100, 1),
			None,
			"a snippet"
		);

		// And a rate the decoder would not answer for, which divides the whole thing by nothing.
		assert_eq!(tempo(&clicks(128.0, 20.0, 44_100), 0, 1), None, "no rate");

		// A decoder emitting `NaN` must not invent onsets: the samples are skipped, so this is
		// the silence it actually is rather than a tempo read off arithmetic.
		assert_eq!(tempo(&[f32::NAN; 441_000], 44_100, 1), None, "not a number");
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
	fn a_scan_that_has_not_landed_yet_draws_as_silence() {
		// Arrange / Act / Assert: the state a player is in for the seconds a long track
		// takes to scan. Zero, not a panic and not a divide by nothing.
		assert_eq!(column_peak(&[], 0, 400), 0.0);
	}
}
