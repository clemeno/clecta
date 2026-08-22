//! The rodio wiring (PLAN §4, §7): one device sink, its mixer, one `rodio::Player` per
//! deck.
//!
//! This is the only module that knows rodio exists. Everything it exposes is either a
//! command to the audio thread or a poll of it — there is no channel back, because rodio
//! does not offer one (PLAN §4) — with one exception at the bottom: `scan`, which decodes
//! a file for its shape rather than for its sound and needs no device at all (PLAN §14a).
//!
//! Every claim encoded here was checked by the audio spike before this file existed, so
//! the surprises (read the duration before `append`; pause before seeking; the sink logs
//! on drop) are already paid for.

use anyhow::{Context, Result};
use rodio::{Decoder, DeviceSinkBuilder, MixerDeviceSink, Player, Source};
use std::fs::File;
use std::path::Path;
use std::time::Duration;

use crate::deck::DeckId;
use crate::waveform::{Scan, Scanner};

/// The audio output, alive for as long as the app can make a sound.
pub struct Engine {
	/// Held, never read: dropping the sink stops every player connected to it.
	_sink: MixerDeviceSink,
	players: [Player; 2],
	description: String,
}

impl Engine {
	/// Open the default output device and connect both players to its mixer.
	///
	/// Fails when there is no usable device, which is a state the app has to survive
	/// rather than crash on — an interface can be unplugged (PLAN §11).
	pub fn new() -> Result<Self> {
		let mut sink =
			DeviceSinkBuilder::open_default_sink().context("no usable audio output device")?;

		// Found by the spike, not the docs: without this, rodio writes a paragraph to
		// stderr when the sink drops at exit (PLAN §7).
		sink.log_on_drop(false);

		let description = format!(
			"{} Hz, {} channels",
			sink.config().sample_rate(),
			sink.config().channel_count()
		);

		// Two players on one mixer — rodio's own central example, and the reason no
		// mixing code is written here (PLAN §2).
		let players = [
			Player::connect_new(sink.mixer()),
			Player::connect_new(sink.mixer()),
		];
		for player in &players {
			player.pause();
		}

		Ok(Self {
			_sink: sink,
			players,
			description,
		})
	}

	/// The device's format, for the status line. Worth showing: it is the first thing to
	/// look at when something sounds wrong.
	pub fn description(&self) -> &str {
		&self.description
	}

	fn player(&self, id: DeckId) -> &Player {
		&self.players[id.index()]
	}

	/// Decode a file into one player, replacing whatever was there, and leave it paused
	/// at 0. Returns the track duration when the decoder can work one out.
	pub fn load(&self, id: DeckId, path: &Path) -> Result<Option<Duration>> {
		let source = decoder(path)?;

		// BEFORE the append below — `append` takes the source by value (PLAN §7). Also
		// before `clear()`, so a file that fails to decode leaves the loaded track alone.
		let duration = source.total_duration();

		let player = self.player(id);
		// `ponytail:` `clear()` blocks until the audio thread has dropped the old source,
		// which is at most one 5 ms control tick. Short enough to call on the GUI thread;
		// if it ever is not, the fix is a fresh `Player` rather than a background task.
		player.clear();
		player.append(source);
		player.pause();

		Ok(duration)
	}

	/// Drop whatever one player holds, leaving it silent and empty. The one transport
	/// change that cannot fail: nothing is decoded and nothing is sought.
	pub fn clear(&self, id: DeckId) {
		self.player(id).clear();
	}

	pub fn play(&self, id: DeckId) {
		self.player(id).play();
	}

	pub fn pause(&self, id: DeckId) {
		self.player(id).pause();
	}

	/// Stop as the transport defines it (PLAN §7): rewind to 0 and pause, keeping the
	/// track loaded. Not `Player::stop()`, which drops the queued source.
	///
	/// Pause FIRST, then seek — rodio applies control changes on a 5 ms tick and
	/// `try_seek` blocks until the audio thread has done the seek, so the other order
	/// leaves the playhead at 5 ms instead of 0. The spike is what caught that.
	pub fn stop(&self, id: DeckId) -> Result<()> {
		let player = self.player(id);
		player.pause();
		player
			.try_seek(Duration::ZERO)
			.context("cannot rewind this stream")?;
		Ok(())
	}

	/// Move the playhead, leaving the transport exactly as it was (PLAN §14).
	///
	/// The deliberate difference from `stop` above is the *absence* of the pause: `stop`
	/// pauses first because it is on its way to a stopped player anyway, and that also
	/// dodges rodio's 5 ms control tick. Here a pause would be audible — a playing player
	/// would gap, and a paused one would need a `play` afterwards it never asked for. The
	/// cost is that the landing point can be a tick out, which is inaudible in a track and
	/// invisible in a 400-pixel strip.
	pub fn seek(&self, id: DeckId, to: Duration) -> Result<()> {
		self.player(id)
			.try_seek(to)
			.context("cannot seek this stream")?;
		Ok(())
	}

	/// Push both gains at once, because they always change together: the crossfader
	/// moves one up as it moves the other down (PLAN §8).
	pub fn set_gains(&self, gains: (f32, f32)) {
		self.players[0].set_volume(gains.0);
		self.players[1].set_volume(gains.1);
	}

	/// The decoder's playhead, polled on the tick (PLAN §4).
	pub fn position(&self, id: DeckId) -> Duration {
		self.player(id).get_pos()
	}

	/// Whether the queue has run out — the only end-of-track signal there is. Meaningful
	/// only for a player that has been given a track.
	pub fn finished(&self, id: DeckId) -> bool {
		self.player(id).empty()
	}
}

/// Open a file as a seekable, measurable stream of samples.
///
/// Shared by playback and by the waveform scan, because the incantation is the part the
/// spike paid for: the *builder* rather than `Decoder::new`, and `with_byte_len`, which is
/// both what makes the stream seekable and what lets `total_duration()` answer at all
/// (PLAN §7). Two call sites getting that subtly different is a bug that only shows up as
/// a track that will not rewind.
fn decoder(path: &Path) -> Result<Decoder<File>> {
	let file = File::open(path).with_context(|| format!("cannot open {}", path.display()))?;
	let byte_len = file
		.metadata()
		.with_context(|| format!("cannot stat {}", path.display()))?
		.len();

	Decoder::builder()
		.with_data(file)
		.with_byte_len(byte_len)
		.with_seekable(true)
		.build()
		.with_context(|| format!("cannot decode {}", path.display()))
}

/// How long a file is, without playing it and without decoding it (PLAN §7a).
///
/// This is the *same* question `load` answers, asked of a file nobody has loaded: the queues
/// show a running time, and a track that has never been near a player still has a length.
/// Building the decoder reads the container's header and stops there — it is an open and a
/// parse, not a decode, which is what makes this cheap enough to do for a whole queue where
/// `peaks` is a job per file.
///
/// One `Option` and no error: a file that will not open and a stream with no length are the
/// same answer to the caller, which is "this row cannot be added up". The caller records it
/// as a measurement that came back empty rather than retrying for ever.
pub fn duration(path: &Path) -> Option<Duration> {
	decoder(path).ok()?.total_duration()
}

/// Scan a whole file: the amplitude array the waveform draws, the music's two edges, and the
/// tempo it beats at.
///
/// A second, independent decode of a file that is already loaded — the playing one cannot
/// be read twice, and reading it would move the playhead. It decodes every sample and
/// throws them all away as it goes, keeping only what `Fold` folds and what `Edges` counts,
/// so the memory cost is the array and nothing else however long the track.
///
/// A free function rather than a method: a scan needs no output device, so the waveform
/// still appears while the app is saying "no audio" (PLAN §11). It takes **seconds** for a
/// long track, which is why every caller runs it off the GUI thread (PLAN §4).
///
/// `ponytail:` the rate and the channel count are read once, before the first sample. rodio
/// allows both to change at a span boundary, which a chained or variable-rate stream can do;
/// a file that did would have its trim scaled by the ratio. Every container the app offers
/// (PLAN §3) is one span, and the fix if that stops being true is to accumulate seconds
/// rather than samples.
pub fn scan(path: &Path) -> Result<Scan> {
	let source = decoder(path)?;
	let rate = source.sample_rate().get();
	let channels = source.channels().get();

	let mut scanner = Scanner::default();
	for sample in source {
		scanner.push(sample);
	}

	Ok(scanner.finish(rate, channels))
}

#[cfg(test)]
mod tests {
	use super::*;

	/// The one test in this module, and the only one that can be: everything else here
	/// needs an output device (PLAN §12). A scan does not — it decodes and throws away —
	/// so the decode path can be checked for real, from a file on disk to the array the
	/// waveform draws and the two edges the handover trims to, with nothing plugged in and
	/// nobody clicking.
	///
	/// The fixture is generated rather than committed: a binary in the repo is a fixture
	/// nobody can read a diff of, and sixteen-bit PCM is a header and some numbers.
	#[test]
	fn a_scan_finds_the_shape_of_a_real_file() {
		// Arrange: one second of digital silence followed by one second at full scale, so
		// the finished array has an obvious answer and a wrong one cannot look plausible.
		const RATE: u32 = 44_100;
		let samples: Vec<i16> = (0..RATE * 2)
			.map(|n| if n < RATE { 0 } else { i16::MAX })
			.collect();

		let path = std::env::temp_dir().join("clecta-scan-test.wav");
		std::fs::write(&path, wav(&samples, RATE)).expect("writing the fixture");

		// Act
		let scanned = scan(&path).expect("scanning the fixture");
		let _ = std::fs::remove_file(&path);

		// Assert: silent through the first half, loud through the second. The halves are
		// compared a little inside their edges, because the column straddling the join
		// legitimately contains both.
		let peaks = scanned.peaks;
		let half = peaks.len() / 2;
		let quietest_loud = peaks[half + 1..].iter().copied().fold(1.0, f32::min);
		let loudest_quiet = peaks[..half - 1].iter().copied().fold(0.0, f32::max);

		assert_eq!(loudest_quiet, 0.0, "the silent second is not silent");
		assert!(
			quietest_loud > 0.9,
			"the loud second reads as {quietest_loud}"
		);

		// And the same join, read as a time rather than as a column: the music starts one
		// second in and runs to the end (PLAN §14c). This is the whole point of measuring the
		// edges per sample — the array above could only say "somewhere in this column".
		let trim = scanned.trim.expect("the fixture is not silent");
		assert_eq!(trim.start, Duration::from_secs(1), "the leader");
		assert_eq!(trim.end, Duration::from_secs(2), "runs to the end");
	}

	/// The other thing that needs no output device: asking a file how long it is (PLAN §7a).
	/// The queues' running times are built entirely out of this answer, so a `None` from a
	/// file that is perfectly readable would leave every total with a `+` on it and no way to
	/// tell why.
	#[test]
	fn a_file_can_be_measured_without_being_played() {
		// Arrange: three seconds of silence, which is a length nothing else could round to.
		const RATE: u32 = 44_100;
		let samples = vec![0i16; RATE as usize * 3];

		let path = std::env::temp_dir().join("clecta-duration-test.wav");
		std::fs::write(&path, wav(&samples, RATE)).expect("writing the fixture");

		// Act
		let measured = duration(&path);
		let _ = std::fs::remove_file(&path);

		// Assert: to the second, which is all the footer shows.
		assert_eq!(measured.map(|length| length.as_secs()), Some(3));

		// And a file that is not there answers rather than failing, because "no length" is
		// what the caller stores either way.
		assert_eq!(duration(&std::env::temp_dir().join("clecta-nothing")), None);
	}

	/// A mono sixteen-bit PCM file: the forty-four byte canonical header, then the samples.
	fn wav(samples: &[i16], rate: u32) -> Vec<u8> {
		let data_len = samples.len() as u32 * 2;
		let mut out = Vec::with_capacity(44 + data_len as usize);

		out.extend(b"RIFF");
		out.extend((36 + data_len).to_le_bytes());
		out.extend(b"WAVEfmt ");
		out.extend(16u32.to_le_bytes()); // the size of the fmt chunk that follows
		out.extend(1u16.to_le_bytes()); // 1 = uncompressed PCM
		out.extend(1u16.to_le_bytes()); // one channel
		out.extend(rate.to_le_bytes());
		out.extend((rate * 2).to_le_bytes()); // bytes per second: rate × block align
		out.extend(2u16.to_le_bytes()); // block align: one channel of sixteen bits
		out.extend(16u16.to_le_bytes()); // bits per sample
		out.extend(b"data");
		out.extend(data_len.to_le_bytes());
		for sample in samples {
			out.extend(sample.to_le_bytes());
		}

		out
	}
}
