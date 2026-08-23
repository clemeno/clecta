//! The plumbing between the decoder and the player (PLAN §7c): what rate a player runs at,
//! and where in the *file* its playhead really is.
//!
//! The player's own position tracking stops being trustworthy the moment the speed is not
//! 1.0: rodio reports the position of the *output*, its seek is scaled by the speed stage,
//! and the anchor it keeps moves whenever the factor changes after a seek. So the app keeps
//! its own: every source is wrapped in a `Stretched` on its way into the player, and the
//! wrapper counts what it hands over — interleaved source samples, the file's own timeline,
//! whatever the speed. One writer on the audio thread, readers on the GUI thread, no locks.
//!
//! Named for what it grows into: the master-tempo stretch (PLAN §7c) lives here too, behind
//! the same wrapper, so the seam between "the file's samples" and "what the player was fed"
//! exists in exactly one place.

use std::sync::Arc;
use std::sync::atomic::{AtomicU32, AtomicU64, Ordering};
use std::time::Duration;

use rodio::source::SeekError;
use rodio::{ChannelCount, Sample, SampleRate, Source};

/// How many samples the wrapper counts locally before it tells `Shared`. An atomic write per
/// sample would put a synchronised operation in the hottest loop in the app; a batch of 1024
/// is ~12 ms of a stereo file, well under the 50 ms tick that reads it.
const FLUSH: u32 = 1024;

/// One player's rate plumbing, shared between the GUI thread and the audio thread.
///
/// Lives on the `Engine`, one per player, and **survives every load**: the slider is a
/// physical fader, so a new track plays at whatever the fader already says — the wrapper
/// around each loaded source holds a clone and writes the playhead, the engine reads it.
#[derive(Debug)]
pub struct Shared {
	/// The playhead, in interleaved source samples — the file's own timeline, whatever the
	/// player's speed, which is what keeps the readout and the strip truthful at 150%.
	position: AtomicU64,
	/// The rate as `f32` bits: 1.0 is the track's own speed, 0.5 half, 2.0 double.
	rate: AtomicU32,
	/// Interleaved samples per second of the loaded file — sample rate × channels. Written
	/// at load, read to turn `position` into a `Duration`.
	per_second: AtomicU32,
}

impl Default for Shared {
	fn default() -> Self {
		Self {
			position: AtomicU64::new(0),
			rate: AtomicU32::new(1.0_f32.to_bits()),
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

	/// The playhead, in the file's own seconds.
	pub fn position(&self) -> Duration {
		let per_second = self.per_second.load(Ordering::Relaxed);
		if per_second == 0 {
			return Duration::ZERO;
		}
		let samples = self.position.load(Ordering::Relaxed);
		Duration::from_secs_f64(samples as f64 / f64::from(per_second))
	}

	/// A new source: remember its format, and put the playhead back at the top. The rate is
	/// deliberately left alone — the fader stays where it was put.
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

/// The source a player is actually given: the decoder, counted.
///
/// Pass-through today — every sample leaves exactly as it arrived — plus the bookkeeping the
/// module doc promises: consumed samples flow into `Shared` in batches, and a seek resets
/// them. The one subtlety is in `try_seek`: the position it receives is already in the
/// file's own timeline, because the player's speed stage multiplies the app's request back
/// up on its way in (see `Engine::seek`).
pub struct Stretched<S> {
	inner: S,
	shared: Arc<Shared>,
	/// Samples counted since the last flush to `shared`.
	pending: u32,
}

impl<S: Source> Stretched<S> {
	pub fn new(inner: S, shared: Arc<Shared>) -> Self {
		shared.loaded(inner.sample_rate().get(), inner.channels().get());
		Self {
			inner,
			shared,
			pending: 0,
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
}

impl<S: Source> Iterator for Stretched<S> {
	type Item = Sample;

	fn next(&mut self) -> Option<Sample> {
		let sample = self.inner.next();
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
	fn current_span_len(&self) -> Option<usize> {
		self.inner.current_span_len()
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
}
