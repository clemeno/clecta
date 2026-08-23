//! The plumbing between the decoder and the player (PLAN §7c): the rate a player runs at —
//! with the pitch, or without it — and where in the *file* its playhead really is.
//!
//! Every source is wrapped in a `Stretched` on its way into the player, and the rate happens
//! *here*, not in rodio's speed stage, which stays at 1.0 for the life of the app. rodio
//! implements speed as a lie about the source's sample rate, and the resampler behind the mixer
//! only re-reads a source's rate when its span runs out — so a source that reports one long
//! span, which this one is (constant rate, constant channels, first sample to last), keeps
//! whatever factor it was appended with for ever and the slider does nothing at all. Reporting
//! fake span boundaries to make rodio look again would buy varispeed at the price of the
//! resampler being thrown away and rebuilt at every boundary, losing the frames it had read
//! ahead each time: a periodic click, and a playhead drifting from what is heard. Doing the
//! rate here costs an interpolation instead, and leaves one place where the tempo happens.
//!
//! The playhead is the other half of the same reason. rodio counts the *output*, which under
//! any rate at all is a different number of samples from the input it was made of — so the
//! wrapper counts the input side itself: interleaved source samples, the file's own timeline,
//! whatever the rate. One writer on the audio thread, readers on the GUI thread, no locks.
//!
//! The lock's stretch is WSOLA — waveform-similarity overlap-add, the standard way to move tempo
//! and leave pitch alone. Output is synthesized a half-window at a time: each block takes a
//! window of input from wherever the rate says the playhead should be, *searches* a little
//! neighbourhood for the offset that lines its waveform up with the end of the previous
//! block, and crossfades the two. The search is what separates it from plain overlap-add,
//! whose misaligned joins phase audibly on anything tonal.

use std::collections::VecDeque;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};
use std::time::Duration;

use rodio::source::SeekError;
use rodio::{ChannelCount, Sample, SampleRate, Source};

/// How many samples the wrapper counts locally before it tells `Shared`. An atomic write per
/// sample would put a synchronised operation in the hottest loop in the app; a batch of 1024
/// is ~12 ms of a stereo file, well under the 50 ms tick that reads it.
const FLUSH: u32 = 1024;

/// Half a stretch window, in frames — the synthesis hop: every block emits this many frames
/// and crossfades over all of them. 1024 frames is ~23 ms at 44.1 kHz, inside the 20–40 ms
/// band where WSOLA neither smears transients (windows too long) nor warbles (too short).
const OVERLAP: usize = 1024;

/// One whole analysis window: the crossfaded half and the half that becomes the next tail.
const WINDOW: usize = 2 * OVERLAP;

/// How many frames of neighbourhood the alignment search sweeps, forward from the ideal
/// position. ~6 ms — enough to find the same phase of anything down to a bass note's period.
const SEARCH: usize = 256;

/// The correlation reads every fourth frame of the first channel. `ponytail:` a 16× cheaper
/// search that is a little worse at aligning the very highest frequencies; full-rate,
/// all-channel correlation is the upgrade if the lock ever sounds rough on hats.
const CORRELATION_STEP: usize = 4;

/// How far from 1.0 the rate has to be before the wrapper re-times anything: re-reading a
/// track at exactly its own speed would cost CPU to make it very slightly worse.
const ENGAGE: f32 = 0.001;

/// One player's rate plumbing, shared between the GUI thread and the audio thread.
///
/// Lives on the `Engine`, one per player, and **survives every load**: the slider and the
/// lock are physical switches, so a new track plays at whatever they already say — the
/// wrapper around each loaded source holds a clone and writes the playhead, the engine
/// reads it.
#[derive(Debug)]
pub struct Shared {
	/// The playhead, in interleaved source samples — the file's own timeline, whatever the
	/// player's speed, which is what keeps the readout and the strip truthful at 150%.
	position: AtomicU64,
	/// The rate as `f32` bits: 1.0 is the track's own speed, 0.5 half, 2.0 double.
	rate: AtomicU32,
	/// The key lock (MT/KL): the rate moves the tempo alone, and the pitch stays put.
	lock: AtomicBool,
	/// Interleaved samples per second of the loaded file — sample rate × channels. Written
	/// at load, read to turn `position` into a `Duration`.
	per_second: AtomicU32,
}

impl Default for Shared {
	fn default() -> Self {
		Self {
			position: AtomicU64::new(0),
			rate: AtomicU32::new(1.0_f32.to_bits()),
			lock: AtomicBool::new(false),
			per_second: AtomicU32::new(0),
		}
	}
}

impl Shared {
	pub fn rate(&self) -> f32 {
		f32::from_bits(self.rate.load(Ordering::Relaxed))
	}

	pub fn set_rate(&self, rate: f32) {
		self.rate.store(rate.to_bits(), Ordering::Relaxed);
	}

	pub fn locked(&self) -> bool {
		self.lock.load(Ordering::Relaxed)
	}

	pub fn set_lock(&self, on: bool) {
		self.lock.store(on, Ordering::Relaxed);
	}

	/// What the wrapper should be doing to the stream right now: nothing, varispeed, or the
	/// lock's stretch. Read per sample, so both switches take effect within one sample.
	fn mode(&self) -> Mode {
		if (self.rate() - 1.0).abs() <= ENGAGE {
			Mode::Through
		} else if self.locked() {
			Mode::Wsola
		} else {
			Mode::Vari
		}
	}

	/// The playhead, in the file's own seconds.
	pub fn position(&self) -> Duration {
		let per_second = self.per_second.load(Ordering::Relaxed);
		if per_second == 0 {
			return Duration::ZERO;
		}
		let samples = self.position.load(Ordering::Relaxed);
		Duration::from_secs_f64(samples as f64 / f64::from(per_second))
	}

	/// A new source: remember its format, and put the playhead back at the top. The rate and
	/// the lock are deliberately left alone — switches stay where they were put.
	fn loaded(&self, rate: u32, channels: u16) {
		self.per_second
			.store(rate.saturating_mul(u32::from(channels)), Ordering::Relaxed);
		self.position.store(0, Ordering::Relaxed);
	}

	/// A seek landed, at this many seconds into the file.
	fn sought(&self, position: Duration) {
		let per_second = self.per_second.load(Ordering::Relaxed);
		let samples = (position.as_secs_f64() * f64::from(per_second)) as u64;
		self.position.store(samples, Ordering::Relaxed);
	}
}

/// Which of the three things the wrapper is doing to the stream.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Mode {
	/// The rate is 1.0: every sample is handed over exactly as it arrived. The overwhelmingly
	/// common case, and it costs a load and a compare per sample.
	Through,
	/// Varispeed: the input is read at `rate` frames per output frame, so the pitch goes with
	/// the tempo — a record pushed faster, which is what the lock being *off* means.
	Vari,
	/// The key lock: the same tempo change, with the pitch left where it was.
	Wsola,
}

/// The source a player is actually given: the decoder, counted — and re-timed, at whatever
/// rate the slider says, with or without the pitch coming along.
///
/// Three modes, decided per sample from `Shared` (see `Mode`). Both re-timing modes share all
/// the bookkeeping below — the input buffer, the anchor, the output queue — and differ only in
/// how they turn `OVERLAP` frames of output out of `OVERLAP × rate` frames of input: `Vari`
/// interpolates, `Wsola` searches and crossfades. The playhead advances by the *input* frames
/// consumed, written as each block's anchor, so the readout moves in ~23 ms steps under a
/// 50 ms tick and always means a place in the file.
///
/// `ponytail:` a mode switch mid-play joins bluntly — up to one synthesized block is
/// dropped, and a stale half-window of buffered input can replay — a one-time click bounded
/// by ~50 ms, against buffering logic that would otherwise have to splice two timelines.
pub struct Stretched<S> {
	inner: S,
	shared: Arc<Shared>,
	/// Interleaved samples per frame, fixed at load like the rate is (see `audio::scan`'s
	/// note on spans).
	channels: usize,
	/// Pass-through: samples counted since the last flush to `shared`.
	pending: u32,
	/// Which mode the last sample was produced in, so a switch is noticed once.
	mode: Mode,
	/// Re-timing: synthesized output waiting to be handed over.
	out: VecDeque<f32>,
	/// Re-timing: buffered input, interleaved, starting at frame `buffer_start`.
	buffer: VecDeque<f32>,
	buffer_start: u64,
	/// The stretch alone: the previous block's second half — what the next block aligns to.
	tail: Vec<f32>,
	/// Re-timing: where the next block's window starts, in input frames. Advances by
	/// `OVERLAP × rate` per block, which is the whole tempo change, in either mode.
	next_input: f64,
	/// The stretch alone: the first block passes through unfaded, there being no tail yet.
	primed: bool,
}

impl<S: Source> Stretched<S> {
	pub fn new(inner: S, shared: Arc<Shared>) -> Self {
		shared.loaded(inner.sample_rate().get(), inner.channels().get());
		let channels = usize::from(inner.channels().get());
		Self {
			inner,
			shared,
			channels,
			pending: 0,
			mode: Mode::Through,
			out: VecDeque::new(),
			buffer: VecDeque::new(),
			buffer_start: 0,
			tail: Vec::new(),
			next_input: 0.0,
			primed: false,
		}
	}

	fn flush(&mut self) {
		if self.pending > 0 {
			self.shared
				.position
				.fetch_add(u64::from(self.pending), Ordering::Relaxed);
			self.pending = 0;
		}
	}

	/// Point the re-timing at wherever the playhead is now: the anchor, and empty buffers.
	fn align(&mut self) {
		self.out.clear();
		self.buffer.clear();
		self.tail.clear();
		self.primed = false;
		let frame = self.shared.position.load(Ordering::Relaxed) / self.channels as u64;
		self.buffer_start = frame;
		self.next_input = frame as f64;
	}

	/// Notice a flipped lock or a rate crossing 1.0 — once, so buffers are reset once.
	fn switch(&mut self, mode: Mode) {
		let was = std::mem::replace(&mut self.mode, mode);
		match (was, mode) {
			// Into a re-timing mode: the count is settled, and the anchor starts wherever the
			// pass-through left the playhead.
			(Mode::Through, _) => {
				self.flush();
				self.align();
			}
			// Out of both: keeps `buffer` — real source samples, drained by the pass-through
			// path and counted there — but drops `out`, which is synthesized audio standing for
			// input the count has already claimed; handing it over would play that input twice.
			(_, Mode::Through) => self.out.clear(),
			// Between the two: the anchor and the buffer carry over untouched, because both
			// modes mean the same thing by them. Only the crossfade's memory is stale.
			_ => {
				self.tail.clear();
				self.primed = false;
			}
		}
	}

	/// Make the buffer cover `count` frames starting at `from`. `false` when the file ran
	/// out first — the partial frame, if any, is padded with silence so indexing stays honest.
	fn buffered(&mut self, from: u64, count: usize) -> bool {
		let wanted = from + count as u64;
		loop {
			let held = self.buffer_start + (self.buffer.len() / self.channels) as u64;
			if held >= wanted {
				return true;
			}
			for _ in 0..self.channels {
				match self.inner.next() {
					Some(sample) => self.buffer.push_back(sample),
					None => {
						while !self.buffer.len().is_multiple_of(self.channels) {
							self.buffer.push_back(0.0);
						}
						return false;
					}
				}
			}
		}
	}

	/// One sample of the buffered input, by source frame and channel. Zero out of range,
	/// which only the padded end of the file can reach.
	fn frame_sample(&self, frame: u64, channel: usize) -> f32 {
		let index = (frame - self.buffer_start) as usize * self.channels + channel;
		self.buffer.get(index).copied().unwrap_or(0.0)
	}

	/// Drop buffered frames before `keep_from` — nothing looks behind the current anchor.
	fn trim(&mut self, keep_from: u64) {
		let frames = keep_from.saturating_sub(self.buffer_start) as usize;
		let drop = (frames * self.channels).min(self.buffer.len());
		self.buffer.drain(..drop);
		self.buffer_start += (drop / self.channels) as u64;
	}

	/// The block's anchor becomes the playhead: input frames, in interleaved samples.
	fn anchored(&self) {
		self.shared.position.store(
			(self.next_input * self.channels as f64) as u64,
			Ordering::Relaxed,
		);
	}

	/// How many input frames one output frame is worth. The floor keeps a slider resting near
	/// 0% from asking for a hundredfold stretch of a player the deck is about to pause anyway.
	fn ratio(&self) -> f64 {
		f64::from(self.shared.rate()).max(0.005)
	}

	/// Synthesize one block into `out` — however the current mode makes blocks — or nothing at
	/// all when the input has run out, which is what ends the stream.
	fn synthesize(&mut self) {
		if self.mode == Mode::Vari {
			self.varispeed();
			return;
		}
		self.wsola();
	}

	/// One hop of varispeed: `OVERLAP` output frames read from the input `ratio` frames apart
	/// and linearly interpolated between the two frames each landing falls between. The same
	/// arithmetic as a record played fast, and the pitch moves with the tempo because of it.
	fn varispeed(&mut self) {
		let ratio = self.ratio();
		let base = self.next_input.floor() as u64;

		// The hops, plus the frame the last interpolation reaches into, plus one for the
		// fractional start. `false` means the file ended inside the window: the loop below then
		// stops at the last real frame rather than interpolating into the silent padding.
		let wanted = (OVERLAP as f64 * ratio).ceil() as usize + 2;
		let complete = self.buffered(base, wanted);
		let held = self.buffer_start + (self.buffer.len() / self.channels) as u64;

		for hop in 0..OVERLAP {
			let at = self.next_input + hop as f64 * ratio;
			let frame = at.floor() as u64;
			if !complete && frame + 1 >= held {
				break;
			}
			let fade = (at - at.floor()) as f32;
			for channel in 0..self.channels {
				let here = self.frame_sample(frame, channel);
				let next = self.frame_sample(frame + 1, channel);
				self.out.push_back(here + (next - here) * fade);
			}
		}

		// Unconditionally, even when the loop stopped early: the anchor has to keep moving past
		// the end of the file, or the next call would ask for the same frames and never finish.
		self.next_input += OVERLAP as f64 * ratio;
		self.anchored();
		self.trim(self.next_input.floor() as u64);
	}

	/// Synthesize one WSOLA block into `out`, or let the tail out when the file ends.
	fn wsola(&mut self) {
		let ratio = self.ratio();
		let base = (self.next_input.floor() as u64).max(self.buffer_start);

		if !self.primed {
			// No tail to align to yet: the first window passes through, half emitted as is,
			// half becoming the tail the next block fades against.
			if !self.buffered(base, WINDOW) {
				self.out.extend(self.tail.drain(..));
				return;
			}
			for frame in 0..OVERLAP {
				for channel in 0..self.channels {
					self.out
						.push_back(self.frame_sample(base + frame as u64, channel));
				}
			}
			self.tail.clear();
			for frame in 0..OVERLAP {
				for channel in 0..self.channels {
					self.tail
						.push(self.frame_sample(base + (OVERLAP + frame) as u64, channel));
				}
			}
			self.primed = true;
		} else {
			if !self.buffered(base, WINDOW + SEARCH) {
				// The tail is real audio that already faded in once: it goes out whole, and
				// the next call finds everything empty and ends the stream.
				self.out.extend(self.tail.drain(..));
				return;
			}

			// The search: which start, within SEARCH frames of ideal, continues the tail's
			// waveform best? Plain correlation, decimated — the peak is broad at these hops.
			let mut best = (f64::MIN, 0u64);
			for offset in 0..SEARCH as u64 {
				let mut score = 0.0_f64;
				for frame in (0..OVERLAP).step_by(CORRELATION_STEP) {
					let held = self.tail[frame * self.channels];
					let candidate = self.frame_sample(base + offset + frame as u64, 0);
					score += f64::from(held) * f64::from(candidate);
				}
				if score > best.0 {
					best = (score, offset);
				}
			}
			let chosen = base + best.1;

			// The crossfade, linear over the whole half-window: the tail hands over to the
			// aligned segment, and the segment's second half becomes the next tail.
			for frame in 0..OVERLAP {
				let fade = frame as f32 / OVERLAP as f32;
				for channel in 0..self.channels {
					let held = self.tail[frame * self.channels + channel];
					let incoming = self.frame_sample(chosen + frame as u64, channel);
					self.out.push_back(held * (1.0 - fade) + incoming * fade);
				}
			}
			for frame in 0..OVERLAP {
				for channel in 0..self.channels {
					self.tail[frame * self.channels + channel] =
						self.frame_sample(chosen + (OVERLAP + frame) as u64, channel);
				}
			}
		}

		// The tempo change is exactly this line: output advanced one hop, input advances
		// `ratio` hops.
		self.next_input += OVERLAP as f64 * ratio;
		self.anchored();
		self.trim(self.next_input.floor() as u64);
	}
}

impl<S: Source> Iterator for Stretched<S> {
	type Item = Sample;

	fn next(&mut self) -> Option<Sample> {
		let mode = self.shared.mode();
		if mode != self.mode {
			self.switch(mode);
		}

		if self.mode != Mode::Through {
			if let Some(sample) = self.out.pop_front() {
				return Some(sample);
			}
			self.synthesize();
			return self.out.pop_front();
		}

		// Pass-through — draining first what a re-timing mode may have left buffered, so those
		// samples play rather than vanish.
		let sample = self.buffer.pop_front().or_else(|| self.inner.next());
		if sample.is_some() {
			self.pending += 1;
			if self.pending == FLUSH {
				self.flush();
			}
		} else {
			// The file ran out: the last partial batch still happened.
			self.flush();
		}
		sample
	}
}

impl<S: Source> Source for Stretched<S> {
	/// `None` deliberately, not the decoder's: the rate makes the output a different length
	/// from the input, so any finite promise would be wrong. `None` reads as "one span,
	/// unknown length", which is what every file the app opens is (see `audio::scan`) — and it
	/// is what lets the resampler behind the mixer be built once and then left alone, because
	/// nothing downstream ever needs to read this source's rate twice (see the header).
	fn current_span_len(&self) -> Option<usize> {
		None
	}

	fn channels(&self) -> ChannelCount {
		self.inner.channels()
	}

	fn sample_rate(&self) -> SampleRate {
		self.inner.sample_rate()
	}

	fn total_duration(&self) -> Option<Duration> {
		self.inner.total_duration()
	}

	fn try_seek(&mut self, position: Duration) -> Result<(), SeekError> {
		self.inner.try_seek(position)?;
		// Only after the decoder agreed: a seek that failed left the playhead where it was,
		// and the model must not claim a move the audio did not make (PLAN §14b).
		self.pending = 0;
		self.shared.sought(position);
		self.align();
		Ok(())
	}
}

#[cfg(test)]
mod tests {
	use super::*;
	use rodio::buffer::SamplesBuffer;
	use std::num::NonZero;

	/// A tiny two-channel source with recognisable samples.
	fn source(samples: Vec<f32>) -> SamplesBuffer {
		SamplesBuffer::new(
			NonZero::new(2).expect("two channels"),
			NonZero::new(4).expect("four frames a second"),
			samples,
		)
	}

	#[test]
	fn the_wrapper_changes_nothing_and_counts_everything() {
		// Arrange: numbered samples, so reordering or dropping one would show.
		let samples: Vec<f32> = (0..20).map(|n| n as f32).collect();
		let shared = Arc::new(Shared::default());
		let wrapped = Stretched::new(source(samples.clone()), Arc::clone(&shared));

		// Act / Assert: pass-through, sample for sample.
		assert_eq!(wrapped.collect::<Vec<f32>>(), samples);

		// Twenty interleaved samples at two channels and four frames a second: 2.5 seconds
		// of the file consumed, counted on the file's own clock.
		assert_eq!(shared.position(), Duration::from_millis(2_500));
	}

	#[test]
	fn a_seek_moves_the_count_to_where_the_decoder_landed() {
		// Arrange: consume a little first, so the reset is visible.
		let shared = Arc::new(Shared::default());
		let mut wrapped = Stretched::new(
			source((0..20).map(|n| n as f32).collect()),
			Arc::clone(&shared),
		);
		for _ in 0..10 {
			wrapped.next();
		}

		// Act
		wrapped
			.try_seek(Duration::from_secs(1))
			.expect("a buffer seeks");

		// Assert: the file's timeline, not the consumption so far.
		assert_eq!(shared.position(), Duration::from_secs(1));

		// And the fader survives a load, because it is a fader: only the playhead resets.
		shared.set_rate(1.5);
		shared.loaded(4, 2);
		assert_eq!(shared.position(), Duration::ZERO);
		assert_eq!(shared.rate(), 1.5);
	}

	/// A 44.1 kHz mono sine, in seconds of it — a signal whose pitch a zero-crossing count can
	/// read back out of whatever the wrapper did to it.
	const RATE: u32 = 44_100;
	const SECONDS: f32 = 3.0;
	const TONE: f32 = 440.0;
	fn tone() -> SamplesBuffer {
		let samples: Vec<f32> = (0..(RATE as f32 * SECONDS) as usize)
			.map(|n| (std::f32::consts::TAU * TONE * n as f32 / RATE as f32).sin())
			.collect();
		SamplesBuffer::new(
			NonZero::new(1).expect("mono"),
			NonZero::new(RATE).expect("the rate"),
			samples,
		)
	}

	/// What that tone sounds like coming out: how long it lasted, and at what pitch.
	fn heard(out: &[f32]) -> (f32, f32) {
		let seconds = out.len() as f32 / RATE as f32;
		let crossings = out
			.windows(2)
			.filter(|pair| pair[0] * pair[1] < 0.0)
			.count();
		(seconds, crossings as f32 / 2.0 / seconds)
	}

	#[test]
	fn the_rate_moves_the_tempo_and_the_pitch_with_it_when_the_lock_is_off() {
		// Arrange: 150%, lock off. This is the plain varispeed case, and the one rodio's own
		// speed stage silently refuses to do for a source that reports a single span — which is
		// why the wrapper does it and the stage is left at 1.0 for ever.
		let shared = Arc::new(Shared::default());
		shared.set_rate(1.5);
		let wrapped = Stretched::new(tone(), Arc::clone(&shared));

		// Act
		let out: Vec<f32> = wrapped.collect();
		let (seconds, pitch) = heard(&out);

		// Assert: three seconds of input came out as two, and the tone went up by the same
		// factor — 440 Hz read faster is 660 Hz, exactly the record-pushed-along sound.
		assert!(
			(seconds - SECONDS / 1.5).abs() < 0.1,
			"3 s at 150% is 2 s of output, got {seconds}"
		);
		assert!(
			(pitch - TONE * 1.5).abs() < TONE * 0.02,
			"a 440 Hz tone at 150% should read 660 Hz, read {pitch}"
		);

		// And the playhead ran the *input's* three seconds, not the output's two.
		assert!(
			shared.position() > Duration::from_secs_f32(2.8),
			"the playhead is the file's own clock, got {:?}",
			shared.position()
		);
	}

	#[test]
	fn the_lock_changes_the_tempo_and_leaves_the_pitch_where_it_was() {
		// Arrange: the same 150%, locked this time.
		let shared = Arc::new(Shared::default());
		shared.set_rate(1.5);
		shared.set_lock(true);
		let wrapped = Stretched::new(tone(), Arc::clone(&shared));

		// Act
		let out: Vec<f32> = wrapped.collect();
		let (seconds, pitch) = heard(&out);

		// Assert: the tempo moved — three seconds of input came out as two of output, give
		// or take the windows at the ends.
		assert!(
			(seconds - SECONDS / 1.5).abs() < 0.1,
			"3 s at 150% is 2 s of output, got {seconds}"
		);

		// …and the pitch did not: the sine still crosses zero 880 times a second. This is the
		// whole claim of the key lock, against the 660 Hz the test above reads off the lock.
		assert!(
			(pitch - TONE).abs() < TONE * 0.02,
			"a 440 Hz tone should stay 440 Hz under the lock, read {pitch}"
		);

		// And the playhead ran the *input's* three seconds, not the output's two.
		assert!(
			shared.position() > Duration::from_secs_f32(2.8),
			"the playhead is the file's own clock, got {:?}",
			shared.position()
		);
	}
}
