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
use rodio::mixer::MixerSource;
use rodio::source::LimitSettings;
use rodio::{Decoder, DeviceSinkBuilder, MixerDeviceSink, Player, Source};
use std::fs::File;
use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use crate::deck::DeckId;
use crate::stretch::{Shared, Stretched};
use crate::waveform::{Scan, Scanner};

/// The audio output, alive for as long as the app can make a sound.
pub struct Engine {
	/// Held, never read: dropping the sink stops every player connected to it.
	_sink: MixerDeviceSink,
	players: [Player; 2],
	/// One per player, surviving every load: the rate slider and the file-timeline playhead
	/// (PLAN §7c). The wrapper around each loaded source holds a clone.
	shared: [Arc<Shared>; 2],
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
		// mixing code is written here (PLAN §2). A mixer of their own, whose sum reaches the
		// device's through the limiter (PLAN Q73). The players join it first: a mixer with
		// nothing in it ends at its first sample, and the device's would then drop it.
		let config = sink.config();
		let (mix, mixed) = rodio::mixer::mixer(config.channel_count(), config.sample_rate());
		let players = [Player::connect_new(&mix), Player::connect_new(&mix)];
		for player in &players {
			player.pause();
		}
		sink.mixer().add(Self::limited(mixed));

		Ok(Self {
			_sink: sink,
			players,
			shared: [Arc::new(Shared::default()), Arc::new(Shared::default())],
			description,
		})
	}

	/// The two players' sum, on its way to the device (PLAN Q73): through rodio's own soft-knee
	/// limiter, at its general-purpose settings. They leave everything under -3 dBFS alone,
	/// which ordinary fader levels never reach, and bend what goes past it towards -1 dBFS —
	/// two loud tracks summed at the crossfader's middle, or a lossy decode past full scale
	/// with a fader up — rather than letting the device flatten it at 1.0.
	fn limited(mix: MixerSource) -> impl Source + Send + 'static {
		mix.limit(LimitSettings::default())
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

		// Wrapped on its way in (PLAN §7c): the wrapper keeps the file-timeline playhead
		// this module's `position` answers with, and resets it to the top for the new track.
		let source = Stretched::new(source, Arc::clone(&self.shared[id.index()]));

		let player = self.player(id);
		// `ponytail:` `clear()` blocks until the audio thread has dropped the old source,
		// which is at most one 5 ms control tick. Short enough to call on the GUI thread;
		// if it ever is not, the fix is a fresh `Player` rather than a background task.
		player.clear();
		player.append(source);
		player.pause();

		Ok(duration)
	}

	/// Set one player's rate (PLAN §7c): 1.0 is the track's own speed. Varispeed, like a
	/// record — the pitch moves with it — unless the key lock holds the pitch.
	///
	/// Notably *not* `Player::set_speed`: rodio's speed stage is left at 1.0 for the life of the
	/// app, because on a single-span source it cannot work at all. The wrapper around the loaded
	/// source reads this within one sample and does the rate itself (PLAN §7c, `stretch`).
	pub fn set_rate(&self, id: DeckId, rate: f32) {
		self.shared[id.index()].set_rate(rate);
	}

	/// Flip one player's key lock (PLAN §7c): on, the rate changes the tempo alone. Same seam as
	/// the rate above — the wrapper reads both and picks which way to re-time.
	pub fn set_lock(&self, id: DeckId, on: bool) {
		self.shared[id.index()].set_lock(on);
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
		// `to` is a place in the *file*, and it arrives at the decoder as one: rodio's speed
		// stage would scale every seek by its factor on the way down, but that factor is 1.0
		// for the life of the app — the rate is the wrapper's job, and so is recording where
		// the seek landed (PLAN §7c).
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
	///
	/// From the wrapper, not from `Player::get_pos` (PLAN §7c): rodio's own count is in the
	/// output's timeline — at 150% it runs slow by exactly that factor, and the anchor its
	/// seeks leave behind moves whenever the rate changes afterwards. The wrapper counts the
	/// file's own samples, which is the number the readout and the strip are drawing.
	pub fn position(&self, id: DeckId) -> Duration {
		self.shared[id.index()].position()
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
	crate::opus::install();
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

	/// A lossy decode goes past full scale wherever the music was mastered to it — 1.22 on an
	/// MP3, 1.20 on an Opus WebM of the same song — and that is sound, not error: the faders
	/// bring it down before anything clips, and the limiter on the mix handles the rest
	/// (PLAN Q73). So nothing is flattened at 1.0. What a corrupt file can decode to is capped
	/// at 1.5, past anything real, and a `NaN` is silence rather than a value that would
	/// poison the limiter's envelope for the rest of the track.
	#[test]
	fn a_decode_keeps_what_is_past_full_scale_up_to_one_and_a_half() {
		// Arrange: a float WAV, which holds whatever it is given.
		let path = std::env::temp_dir().join("clecta-past-full-scale.wav");
		std::fs::write(&path, float_wav(&[1.2, -1.2, 3.0, -3.0, f32::NAN, 0.5])).expect("writing");

		// Act
		let decoded: Vec<f32> = decoder(&path).expect("opening the fixture").collect();
		let _ = std::fs::remove_file(&path);

		// Assert
		assert_eq!(decoded, [1.2, -1.2, 1.5, -1.5, 0.0, 0.5]);
	}

	/// The two players summed and limited, as the engine sends them to the device. A mix
	/// that stays under the knee comes out exactly as it went in, so at ordinary fader levels
	/// the limiter is not there at all; one past full scale, as two loud tracks summed at the
	/// crossfader's middle are, is brought back under it once the limiter has caught up,
	/// bent rather than flattened (PLAN Q73).
	#[test]
	fn the_mix_is_limited_only_when_it_is_loud() {
		use rodio::source::SineWave;

		// Arrange: a second of a 440 Hz tone at each level.
		let mix = |level: f32| {
			let (mixer, mixed) = rodio::mixer::mixer(
				std::num::NonZero::new(1).expect("one"),
				std::num::NonZero::new(48_000).expect("a rate"),
			);
			mixer.add(
				SineWave::new(440.0)
					.amplify(level)
					.take_duration(Duration::from_secs(1)),
			);
			Engine::limited(mixed).collect::<Vec<f32>>()
		};
		let peak = |samples: &[f32]| samples.iter().fold(0.0f32, |peak, s| peak.max(s.abs()));

		// Act
		let quiet = mix(0.4);
		let loud = mix(1.5);

		// Assert: the quiet tone untouched; the loud one under full scale after 50 ms.
		let tone: Vec<f32> = SineWave::new(440.0).amplify(0.4).take(48_000).collect();
		assert!(
			quiet
				.iter()
				.zip(&tone)
				.all(|(out, sine)| (out - sine).abs() < 1e-6),
			"the limiter touched a quiet mix"
		);
		assert!(
			peak(&loud[2_400..]) <= 1.0,
			"a loud mix still peaks at {}",
			peak(&loud[2_400..])
		);
	}

	/// The shape of almost every downloaded video: the picture is the first track and the
	/// sound the second (PLAN §3). The decoder plays the sound, so the sound is what has to
	/// be measured — rodio read the length off the picture's frame count instead, and an
	/// MP4 of 3:48 showed 1:06. The fixture's video counts in a different unit from its
	/// audio exactly so that mistake cannot come out right by luck.
	#[test]
	fn a_video_first_mp4_is_as_long_as_its_sound() {
		// Arrange
		let path = std::env::temp_dir().join("clecta-video-first-length.mp4");
		std::fs::write(&path, video_first_mp4(Sound::default())).expect("writing the fixture");

		// Act and assert: one second exactly, the audio's own 8 000 frames over its own 8 kHz,
		// and every one of those frames is sound — the picture's bytes, handed to the audio
		// decoder, would have come out as samples of their own.
		assert_one_timeline(&path, Duration::from_secs(1));
		let _ = std::fs::remove_file(&path);
	}

	/// An MP4 whose edit list says where its sound is. An AAC encoder puts 1 024 frames of
	/// priming in front, and the file's one edit plays the media from after them for the
	/// length of the sound alone; rodio played and counted the priming, 21 ms of near-silence
	/// at the start of every AAC file (PLAN Q72). Here the edit skips the first 800 frames of
	/// PCM that counts up from zero and plays 800 ms, so the first sample heard says where the
	/// sound starts and the length says where it ends.
	#[test]
	fn an_mp4_plays_what_its_edit_list_says() {
		// Arrange
		let path = std::env::temp_dir().join("clecta-edit-list.mp4");
		std::fs::write(
			&path,
			video_first_mp4(Sound {
				edit: Some((800, 800)),
				..Sound::default()
			}),
		)
		.expect("writing the fixture");

		// Act
		let first = decoder(&path)
			.expect("opening the fixture")
			.next()
			.map(|sample| (sample * 32_768.0).round() as i32);

		// Assert
		assert_eq!(first, Some(800), "the first sample heard");
		assert_one_timeline(&path, Duration::from_millis(800));
		let _ = std::fs::remove_file(&path);
	}

	/// A picture track's edit list is not priming: a video cut without re-encoding keeps one
	/// that skips the picture's start. Read as a delay, it pushed a seek near the end past the
	/// picture's last frame, and the whole seek failed, which the app shows as a stream it
	/// cannot seek (PLAN Q72).
	#[test]
	fn a_picture_tracks_edit_does_not_stop_a_seek_near_the_end() {
		// Arrange: the picture's edit skips its first half second.
		let path = std::env::temp_dir().join("clecta-picture-edit.mp4");
		std::fs::write(
			&path,
			video_first_mp4(Sound {
				picture_edit: Some((500, 500)),
				..Sound::default()
			}),
		)
		.expect("writing the fixture");
		let mut source = decoder(&path).expect("opening the fixture");

		// Act
		let sought = source.try_seek(Duration::from_millis(950));
		let remaining = source.count();
		let _ = std::fs::remove_file(&path);

		// Assert: 50 ms of 8 kHz sound left.
		assert!(sought.is_ok(), "the seek failed: {sought:?}");
		assert_eq!(remaining, 400, "what the seek left");
	}

	/// A picture that ends before its sound, as a downloaded video's often does by a frame or
	/// so (228.32 s of picture against 228.44 s of sound in the user's). symphonia's MP4
	/// reader seeks every track along with the sound, and a seek past the picture's end failed
	/// the sound's seek with it: the last fraction of a second of such a file could not be
	/// sought to at all, and the app said it could not seek the stream (PLAN Q72).
	#[test]
	fn a_seek_past_the_end_of_a_shorter_picture_still_lands() {
		// Arrange: 800 ms of picture over a second of sound.
		let path = std::env::temp_dir().join("clecta-short-picture.mp4");
		std::fs::write(
			&path,
			video_first_mp4(Sound {
				picture_frames: 20,
				..Sound::default()
			}),
		)
		.expect("writing the fixture");
		let mut source = decoder(&path).expect("opening the fixture");

		// Act
		let sought = source.try_seek(Duration::from_millis(900));
		let landed = source
			.next()
			.map(|sample| (sample * 32_768.0).round() as i32);
		let rest = source.count();
		let _ = std::fs::remove_file(&path);

		// Assert: 900 ms in is frame 7 200, and 799 more follow it.
		assert!(sought.is_ok(), "the seek failed: {sought:?}");
		assert_eq!(landed, Some(7_200), "the seek landed somewhere else");
		assert_eq!(rest, 799, "what the seek left after that sample");
	}

	/// The same seek, read where the picture is still in sight: the MP4 reader hands over the
	/// earliest packet of any track, so a picture the seek could not move would hand over all
	/// it has before the sound's next. rodio skips it either way; a reader of both does not.
	#[test]
	fn a_seek_past_the_end_of_a_shorter_picture_reads_the_sound_next() {
		use symphonia::core::formats::{FormatReader, SeekMode, SeekTo};
		use symphonia::core::io::MediaSourceStream;
		use symphonia::core::units::Time;
		use symphonia::default::formats::IsoMp4Reader;

		// Arrange: 800 ms of picture, track 0, over a second of sound, track 1.
		let bytes = video_first_mp4(Sound {
			picture_frames: 20,
			..Sound::default()
		});
		let stream =
			MediaSourceStream::new(Box::new(std::io::Cursor::new(bytes)), Default::default());
		let mut reader =
			IsoMp4Reader::try_new(stream, &Default::default()).expect("opening the fixture");

		// Act
		let sought = reader.seek(
			SeekMode::Accurate,
			SeekTo::Time {
				time: Time::new(0, 0.9),
				track_id: Some(1),
			},
		);
		let next = reader.next_packet().map(|packet| packet.track_id());

		// Assert
		assert!(sought.is_ok(), "the seek failed: {sought:?}");
		assert_eq!(
			next.ok(),
			Some(1),
			"the packet after the seek is not the sound's"
		);
	}

	/// The same edit on a sound whose clock ticks twice a frame, as an MP4's can: its timescale
	/// need not be its sample rate, and in HE-AAC it is twice the rate symphonia decodes at.
	/// The trims arrive in ticks and the decoder's frames are frames, and comparing the two
	/// skipped the trims, so the priming played while the length and the seeks left it out.
	#[test]
	fn an_mp4_edit_list_holds_when_the_clock_is_not_the_rate() {
		// Arrange: 1 600 ticks is the same 800 frames.
		let path = std::env::temp_dir().join("clecta-edit-list-ticks.mp4");
		std::fs::write(
			&path,
			video_first_mp4(Sound {
				edit: Some((1_600, 800)),
				scale: 2 * MP4_RATE,
				..Sound::default()
			}),
		)
		.expect("writing the fixture");

		// Act
		let first = decoder(&path)
			.expect("opening the fixture")
			.next()
			.map(|sample| (sample * 32_768.0).round() as i32);

		// Assert
		assert_eq!(first, Some(800), "the first sample heard");
		assert_one_timeline(&path, Duration::from_millis(800));
		let _ = std::fs::remove_file(&path);
	}

	/// The same on a clock that does not divide the rate and is coarser than it: a packet of
	/// 100 frames is 37.5 ticks of 3 kHz, stored as 37, which read back as 98.67 frames.
	/// Rounded down to 98, or to the nearest, 99, they did not match the 100 the decoder gave,
	/// so the trims were taken as done and the priming played; and a packet the edit skips
	/// whole, 37 ticks, kept a frame of the 100 when its trim was read as 99 of them.
	#[test]
	fn an_mp4_edit_list_holds_when_the_clock_does_not_divide_the_rate() {
		// Arrange: the edit skips two packets, 74 ticks.
		let path = std::env::temp_dir().join("clecta-edit-list-rounded.mp4");
		std::fs::write(
			&path,
			video_first_mp4(Sound {
				edit: Some((74, 800)),
				scale: 3_000,
				frames: 100,
				..Sound::default()
			}),
		)
		.expect("writing the fixture");

		// Act
		let first = decoder(&path)
			.expect("opening the fixture")
			.next()
			.map(|sample| (sample * 32_768.0).round() as i32);
		let _ = std::fs::remove_file(&path);

		// Assert
		assert_eq!(first, Some(200), "the first sample heard");
	}

	/// An edit that starts and ends partway through a packet, as nearly every AAC file's does:
	/// its packets hold 1 024 frames, and Apple's encoders prime with 2 112, two packets and a
	/// part. The decoder then keeps the end of one packet and the start of another, and in
	/// stereo every frame it keeps or drops is two samples.
	#[test]
	fn an_mp4_edit_can_start_and_end_partway_through_a_stereo_packet() {
		// Arrange: packets of 100 frames, and an edit from frame 150 to frame 6 550.
		let path = std::env::temp_dir().join("clecta-edit-list-stereo.mp4");
		std::fs::write(
			&path,
			video_first_mp4(Sound {
				edit: Some((150, 800)),
				channels: 2,
				frames: 100,
				..Sound::default()
			}),
		)
		.expect("writing the fixture");
		let frame = |source: &mut dyn Iterator<Item = f32>| -> Vec<i32> {
			source
				.take(2)
				.map(|sample| (sample * 32_768.0).round() as i32)
				.collect()
		};

		// Act
		let first = frame(&mut decoder(&path).expect("opening the fixture"));
		let mut sought = decoder(&path).expect("opening the fixture");
		let seek = sought.try_seek(Duration::from_millis(400));
		let landed = frame(&mut sought);

		// Assert: left is `n` and right is `-n`; 400 ms in is 3 200 frames past frame 150.
		assert_eq!(first, [150, -150], "the first frame heard");
		assert!(seek.is_ok(), "the seek failed: {seek:?}");
		assert_eq!(landed, [3_350, -3_350], "the frame a seek landed on");
		assert_one_timeline(&path, Duration::from_millis(800));
		let _ = std::fs::remove_file(&path);
	}

	/// An edit a few ticks longer than the sound after its priming: the movie's clock counts
	/// whole milliseconds, or QuickTime's 1/600 s, and a muxer may round the edit's length up
	/// to it. The edit was then refused as a whole, and the priming played (PLAN Q72). Here
	/// 901 ms from frame 800 would end 8 frames, one millisecond, past the 8 000 there are.
	#[test]
	fn an_mp4_edit_that_runs_past_its_sound_still_skips_the_priming() {
		// Arrange
		let path = std::env::temp_dir().join("clecta-edit-list-long.mp4");
		std::fs::write(
			&path,
			video_first_mp4(Sound {
				edit: Some((800, 901)),
				..Sound::default()
			}),
		)
		.expect("writing the fixture");

		// Act
		let first = decoder(&path)
			.expect("opening the fixture")
			.next()
			.map(|sample| (sample * 32_768.0).round() as i32);

		// Assert: from frame 800 to the last, 900 ms.
		assert_eq!(first, Some(800), "the first sample heard");
		assert_one_timeline(&path, Duration::from_millis(900));
		let _ = std::fs::remove_file(&path);
	}

	/// An edit that runs further past the sound than the movie's clock can round is not
	/// rounding: either the media's header undercounts its sound, and cutting at it would cut
	/// sound, or the edit is wrong. The media then plays whole, priming and all, as a header
	/// that undercounts always has (PLAN Q72). Here 1 000 ms from frame 800 would end 100 ms,
	/// a hundred ticks, past the sound.
	#[test]
	fn an_mp4_edit_far_past_its_sound_plays_the_media_whole() {
		// Arrange
		let path = std::env::temp_dir().join("clecta-edit-list-wrong.mp4");
		std::fs::write(
			&path,
			video_first_mp4(Sound {
				edit: Some((800, 1_000)),
				..Sound::default()
			}),
		)
		.expect("writing the fixture");

		// Act
		let first = decoder(&path)
			.expect("opening the fixture")
			.next()
			.map(|sample| (sample * 32_768.0).round() as i32);

		// Assert
		assert_eq!(first, Some(0), "the first sample heard");
		assert_one_timeline(&path, Duration::from_secs(1));
		let _ = std::fs::remove_file(&path);
	}

	/// The same file, sought. rodio clamped every seek to the wrong length above, and then
	/// placed it on the video track's clock — so on the real file a seek to 2:30 landed at
	/// 1:06, and one to 0:30 a frame early. The fixture's samples count up from zero, so the
	/// first one after the seek says exactly where it landed.
	#[test]
	fn a_seek_in_a_video_first_mp4_lands_where_it_was_aimed() {
		// Arrange
		let path = std::env::temp_dir().join("clecta-video-first-seek.mp4");
		std::fs::write(&path, video_first_mp4(Sound::default())).expect("writing the fixture");
		let mut source = decoder(&path).expect("opening the fixture");

		// Act
		let sought = source.try_seek(Duration::from_millis(500));
		let landed = source
			.next()
			.map(|sample| (sample * 32_768.0).round() as i32);
		drop(source);
		let _ = std::fs::remove_file(&path);

		// Assert: half a second in is frame 4 000, whose sample is 4 000.
		assert!(sought.is_ok(), "the seek failed: {sought:?}");
		assert_eq!(landed, Some(4_000), "the seek landed somewhere else");
	}

	/// A seek to the first sample of a fragment. Q72 starts a seek's decode one sample early,
	/// which an AAC decoder needs, and that sample is the last of the fragment before; read
	/// from the target's own fragment, its number underflowed — a panic in a debug build, a
	/// failed seek in a release one.
	#[test]
	fn a_seek_to_the_start_of_an_mp4_fragment_lands_there() {
		// Arrange
		let path = std::env::temp_dir().join("clecta-fragmented.mp4");
		std::fs::write(&path, fragmented_mp4()).expect("writing the fixture");
		let mut source = decoder(&path).expect("opening the fixture");

		// Act: the second fragment starts at frame 4 000, half a second in.
		let sought = source.try_seek(Duration::from_millis(500));
		let landed = source
			.next()
			.map(|sample| (sample * 32_768.0).round() as i32);
		let rest = source.count();
		let _ = std::fs::remove_file(&path);

		// Assert
		assert!(sought.is_ok(), "the seek failed: {sought:?}");
		assert_eq!(landed, Some(4_000), "the seek landed somewhere else");
		assert_eq!(rest, 3_999, "what the seek left after that sample");
	}

	/// A WebM as video sites serve it: the picture first, the sound in Opus. symphonia 0.5
	/// demuxes WebM but has no Opus decoder, so the row was offered and every load failed
	/// (PLAN Q71). One second of Opus is fifty 20 ms packets, and all of it is decoded —
	/// the encoder's 312-sample pre-skip included, so the decode, the container's length and
	/// every seek count from the same zero.
	#[test]
	fn a_video_first_opus_webm_is_as_long_as_its_sound() {
		// Arrange
		let path = std::env::temp_dir().join("clecta-video-first.webm");
		std::fs::write(&path, video_first_webm(1_000, Some(0), 0)).expect("writing the fixture");

		// Act and assert
		assert_one_timeline(&path, Duration::from_secs(1));
		let _ = std::fs::remove_file(&path);
	}

	/// A WebM whose header ends partway through its last packet, as every encoder's does: Opus
	/// codes whole 20 ms frames, so the last one is padded, and Matroska says where the sound
	/// ends only by its length. rodio played the padding (PLAN Q72); it now stops where the
	/// header says.
	#[test]
	fn a_webm_ends_where_its_header_says() {
		// Arrange: fifty packets, and a header that claims 990 ms of them.
		let path = std::env::temp_dir().join("clecta-short-header.webm");
		std::fs::write(&path, video_first_webm(990, Some(0), 0)).expect("writing the fixture");

		// Act and assert
		assert_one_timeline(&path, Duration::from_millis(990));
		let _ = std::fs::remove_file(&path);
	}

	/// A WebM with no cues, as a WebM written to a pipe can be: symphonia then has nothing to
	/// seek back with, only forward, to the first packet at or after the target. Q72's step
	/// back cannot help there and must not walk on — it did, a packet per try, and landed
	/// 0.3 s late in a long file. The seek lands at most one packet late, as it always has.
	#[test]
	fn a_webm_without_cues_seeks_at_most_one_packet_late() {
		// Arrange
		let path = std::env::temp_dir().join("clecta-no-cues.webm");
		std::fs::write(&path, video_first_webm(1_000, None, 0)).expect("writing the fixture");
		let mut source = decoder(&path).expect("opening the fixture");

		// Act: 495 ms is 5 ms into the packet that starts at 480.
		let sought = source.try_seek(Duration::from_millis(495));
		let remaining = source.count();
		let _ = std::fs::remove_file(&path);

		// Assert: it lands on the next packet, at 500 ms, with half a second of sound left.
		assert!(sought.is_ok(), "the seek failed: {sought:?}");
		assert_eq!(remaining, 25 * 960, "what the seek left");
	}

	/// The same file sought into its last packet but one: the seek lands on the last, and
	/// stepping back from there finds the end of the stream, not a packet. That is no reason
	/// to fail the seek; it keeps the late landing.
	#[test]
	fn a_webm_without_cues_seeks_near_its_end() {
		// Arrange
		let path = std::env::temp_dir().join("clecta-no-cues-end.webm");
		std::fs::write(&path, video_first_webm(1_000, None, 0)).expect("writing the fixture");
		let mut source = decoder(&path).expect("opening the fixture");

		// Act: 965 ms is 5 ms into the packet that starts at 960.
		let sought = source.try_seek(Duration::from_millis(965));
		let remaining = source.count();
		let _ = std::fs::remove_file(&path);

		// Assert: it lands on the last packet, at 980 ms.
		assert!(sought.is_ok(), "the seek failed: {sought:?}");
		assert_eq!(remaining, 960, "what the seek left");
	}

	/// A WebM whose one cue says its cluster starts 40 ms in, while its sound starts at 0, as
	/// cues on a picture's keyframes can. A seek to 45 ms lands late, at 60, and stepping back
	/// from there asks for 30, below every cue, which Matroska refuses before it moves. The
	/// seek failed then, though the sound had already moved, and the app's playhead stayed
	/// behind (PLAN Q72). It keeps the late landing, as upstream did.
	#[test]
	fn a_webm_whose_first_cue_is_late_still_seeks_near_its_start() {
		// Arrange
		let path = std::env::temp_dir().join("clecta-late-cue.webm");
		std::fs::write(&path, video_first_webm(1_000, Some(40), 0)).expect("writing the fixture");
		let mut source = decoder(&path).expect("opening the fixture");

		// Act
		let sought = source.try_seek(Duration::from_millis(45));
		let remaining = source.count();
		let _ = std::fs::remove_file(&path);

		// Assert: from 60 ms, forty-seven packets.
		assert!(sought.is_ok(), "the seek failed: {sought:?}");
		assert_eq!(remaining, 47 * 960, "what the seek left");
	}

	/// A WebM whose sound starts 2 s into the container's clock, as a file cut from a longer
	/// stream can keep its timestamps: the header's duration runs from zero, and every block is
	/// 2 s on. The app's timeline starts at the first sound, so it is one second long and a
	/// seek half a second in lands half a second into the sound. rodio measured three seconds,
	/// refused every seek into the first two and landed later ones 2 s early (PLAN Q72).
	#[test]
	fn a_webm_that_starts_late_is_timed_from_its_first_sound() {
		// Arrange
		let path = std::env::temp_dir().join("clecta-late-start.webm");
		std::fs::write(&path, video_first_webm(1_000, Some(0), 2_000))
			.expect("writing the fixture");

		// Act and assert
		assert_one_timeline(&path, Duration::from_secs(1));
		let _ = std::fs::remove_file(&path);
	}

	/// The same padding in Ogg, which states it exactly: the last page's granule position
	/// stops short of what its packets decode to (RFC 7845 §4.4), and the demuxer turns that
	/// into a trim on the last packet. A decoder applies the trims it is handed, as
	/// symphonia's own do; the Opus one did not, and played 10 ms of padding (PLAN Q72).
	#[test]
	fn an_ogg_opus_ends_where_its_last_page_says() {
		use opus_pure::{OggOpusWriter, OpusHead};

		// Arrange: fifty packets, the last one declared 480 samples long rather than 960.
		let packets = opus_packets();
		let mut writer =
			OggOpusWriter::new(Vec::new(), OpusHead::new(1, 48_000).expect("a header"))
				.expect("a writer");
		for packet in &packets[..49] {
			writer.write_packet(packet).expect("writing a packet");
		}
		writer
			.write_packet_with_duration(&packets[49], 480)
			.expect("writing the last packet");
		let path = std::env::temp_dir().join("clecta-end-trim.ogg");
		std::fs::write(&path, writer.finish().expect("finishing the stream"))
			.expect("writing the fixture");

		// Act and assert
		assert_one_timeline(&path, Duration::from_millis(990));
		let _ = std::fs::remove_file(&path);
	}

	/// What every file must agree with itself on: the length its header gives, the sound a
	/// decode gives, and what a seek to the middle leaves. Each timing bug so far was two of
	/// those disagreeing — a length read off the picture's track (PLAN Q69), a pre-skip
	/// dropped from the start but not after a seek (Q71), padding played past the header's
	/// end (Q72). The fixtures have an even frame count, so half is exact.
	fn assert_one_timeline(path: &Path, length: Duration) {
		assert_eq!(duration(path), Some(length), "the header's length");

		let source = decoder(path).expect("opening the fixture");
		let per_second =
			u128::from(source.sample_rate().get()) * u128::from(source.channels().get());
		let samples = (length.as_nanos() * per_second / 1_000_000_000) as usize;
		assert_eq!(source.count(), samples, "the decoded sound");

		let mut sought = decoder(path).expect("opening the fixture");
		let seek = sought.try_seek(length / 2);
		assert!(seek.is_ok(), "the seek failed: {seek:?}");
		assert_eq!(
			sought.count(),
			samples / 2,
			"what a seek to the middle left"
		);
	}

	/// One second of real Opus, mono: fifty 20 ms packets encoded from a tone with a burst of
	/// noise every quarter second. Real packets rather than the three-byte silence one,
	/// because silence skips nearly all of a decoder, and the bursts are transients, which take
	/// CELT down its short-block paths. They do not reproduce the crash that ruled out the first
	/// Opus crate tried (PLAN Q71) — only a real file did, which is what
	/// `scripts/probe-formats.sh` takes as arguments.
	fn opus_packets() -> Vec<Vec<u8>> {
		use opus_pure::{Application, MAX_PACKET_BYTES, OpusEncoder};

		let mut encoder = OpusEncoder::new(48_000, 1, Application::Audio).expect("an encoder");
		let mut noise = 1u32; // a fixed generator, so every run encodes the same file
		let pcm: Vec<f32> = (0..48_000u32)
			.map(|n| {
				noise = noise.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
				let tone = (n as f32 * 440.0 * std::f32::consts::TAU / 48_000.0).sin() * 0.3;
				let burst = if n % 12_000 < 480 {
					(noise >> 8) as f32 / (1 << 24) as f32 - 0.5
				} else {
					0.0
				};
				tone + burst
			})
			.collect();

		let mut packet = vec![0; MAX_PACKET_BYTES];
		pcm.as_chunks::<960>()
			.0
			.iter()
			.map(|frame| {
				let size = encoder.encode(frame, 960, &mut packet).expect("encoding");
				packet[..size].to_vec()
			})
			.collect()
	}

	/// A WebM with the video first: a VP9 track of opaque frames, then mono Opus — the fifty
	/// packets of `opus_packets`, starting `start_ms` into the container's clock, under a header
	/// that claims `length_ms` of sound after that. Given `cue_ms`, one cue indexes the cluster
	/// as starting that far into the sound, as cues on a picture's keyframes can say of a
	/// cluster whose sound starts earlier. No block carries a duration and the track no default
	/// one, so symphonia seeks to the first block at or after a target. Built by hand for the
	/// same reason `video_first_mp4` is. Each element's size is written in the eight-byte
	/// form, which EBML allows for any size and which keeps the arithmetic out.
	fn video_first_webm(length_ms: u32, cue_ms: Option<u32>, start_ms: u32) -> Vec<u8> {
		const PRE_SKIP: u16 = 312;

		let uint = |n: u64| n.to_be_bytes();
		let opus_head = [
			&b"OpusHead"[..],
			&[1, 1],                  // version 1, one channel
			&PRE_SKIP.to_le_bytes(),  // samples to drop at the start, at 48 kHz
			&48_000u32.to_le_bytes(), // the input's rate, for information only
			&[0, 0, 0],               // no output gain, channel mapping family 0
		]
		.concat();

		let ebml = webm_element(
			&[0x1a, 0x45, 0xdf, 0xa3],
			&[&webm_element(&[0x42, 0x82], &[b"webm"])],
		);
		let info = webm_element(
			&[0x15, 0x49, 0xa9, 0x66],
			&[
				&webm_element(&[0x2a, 0xd7, 0xb1], &[&uint(1_000_000)]), // timestamps in ms
				// The segment's duration, which Matroska counts from zero, not from the start.
				&webm_element(
					&[0x44, 0x89],
					&[&f64::from(start_ms + length_ms).to_be_bytes()],
				),
			],
		);
		let track = |number: u64, kind: u64, codec: &[u8], extra: &[&[u8]]| {
			let head = [
				webm_element(&[0xd7], &[&uint(number)]),
				webm_element(&[0x73, 0xc5], &[&uint(number)]),
				webm_element(&[0x83], &[&uint(kind)]),
				webm_element(&[0x86], &[codec]),
			]
			.concat();
			webm_element(&[0xae], &[&head, &extra.concat()])
		};
		let tracks = webm_element(
			&[0x16, 0x54, 0xae, 0x6b],
			&[
				&track(1, 1, b"V_VP9", &[&webm_element(&[0xe0], &[])]),
				&track(
					2,
					2,
					b"A_OPUS",
					&[
						&webm_element(&[0x63, 0xa2], &[&opus_head]),
						&webm_element(
							&[0xe1],
							&[
								&webm_element(&[0xb5], &[&48_000f64.to_be_bytes()]),
								&webm_element(&[0x9f], &[&uint(1)]),
							],
						),
					],
				),
			],
		);
		let cues = |cluster: u64, time: u32| {
			let position = webm_element(
				&[0xb7],
				&[
					&webm_element(&[0xf7], &[&uint(2)]),
					&webm_element(&[0xf1], &[&uint(cluster)]),
				],
			);
			webm_element(
				&[0x1c, 0x53, 0xbb, 0x6b],
				&[&webm_element(
					&[0xbb],
					&[&webm_element(&[0xb3], &[&uint(time.into())]), &position],
				)],
			)
		};
		let block = |track: u8, at: i16, frame: &[u8]| {
			webm_element(
				&[0xa3],
				&[&[0x80 | track], &at.to_be_bytes(), &[0x80], frame],
			)
		};
		let mut cluster = vec![webm_element(&[0xe7], &[&uint(start_ms.into())])];
		for (at, packet) in (0..).step_by(20).zip(opus_packets()) {
			if at % 40 == 0 {
				cluster.push(block(1, at, &[0; 4]));
			}
			cluster.push(block(2, at, &packet));
		}
		let cluster = webm_element(&[0x1f, 0x43, 0xb6, 0x75], &[&cluster.concat()]);

		// The cue points at the cluster from the start of the segment's body, and the cues
		// before the cluster are the same size whatever that number is.
		let cues = cue_ms.map_or_else(Vec::new, |cue_ms| {
			let time = start_ms + cue_ms;
			cues(
				(info.len() + tracks.len() + cues(0, time).len()) as u64,
				time,
			)
		});
		let segment = webm_element(
			&[0x18, 0x53, 0x80, 0x67],
			&[&info, &tracks, &cues, &cluster],
		);

		[ebml, segment].concat()
	}

	/// An EBML element: its ID as written, an eight-byte size, then its body.
	fn webm_element(id: &[u8], parts: &[&[u8]]) -> Vec<u8> {
		let body = parts.concat();
		let mut size = (body.len() as u64).to_be_bytes();
		size[0] = 0x01; // the width marker: seven bytes of size follow
		[id, &size, &body].concat()
	}

	/// A probe, for files the suite cannot ship: what the app learns about each file named in
	/// `CLECTA_PROBE` (a path list, `;`-separated on Windows and `:`-separated elsewhere) —
	/// the length from the header and from a full decode, where four seeks land, and the
	/// scan's answers — so two files that should agree can be read side by side. It found Q69,
	/// an MP4 whose header said 66 s and whose decode said 228, and Q72's padding and late
	/// seeks. It fails when a file disagrees with itself the way `assert_one_timeline` checks
	/// a fixture, within `TOLERANCE`, after printing every file; `scripts/probe-formats.sh`
	/// runs it on a tone per format, a few shaped the ways that broke, and any files given.
	///
	/// `CLECTA_PROBE="a.mp3;a.mp4" cargo test probe_files -- --ignored --nocapture`
	#[test]
	#[ignore = "reads the files named in CLECTA_PROBE"]
	fn probe_files() {
		// Matroska's timestamps are whole milliseconds, and ffmpeg's Matroska length can end a
		// few past the sound (3 ms on a stereo Vorbis tone): no decode can reach it, and a
		// landing, measured back from the decode's end, reads early by as much. Everything
		// Q72 fixed was 6.5 ms or more.
		const TOLERANCE: f64 = 0.005;

		let paths = std::env::var_os("CLECTA_PROBE").expect("CLECTA_PROBE names no files");
		let mut wrong = Vec::new();
		for path in std::env::split_paths(&paths) {
			// A file still being probed after five minutes is stuck — a seek that never returns
			// — and fails the probe rather than hanging it. A healthy hour of stereo AAC probes
			// in 26 s in a debug build, so five minutes is some eleven hours of sound. `_probed`
			// drops at the end of the file, and the watchdog then ends quietly.
			let (_probed, watched) = std::sync::mpsc::channel::<()>();
			let name = path.display().to_string();
			std::thread::spawn(move || {
				let five_minutes = Duration::from_secs(300);
				if let Err(std::sync::mpsc::RecvTimeoutError::Timeout) =
					watched.recv_timeout(five_minutes)
				{
					eprintln!("{name}: still probing after five minutes, so stuck");
					std::process::exit(1);
				}
			});

			let source = decoder(&path).expect("opening the file");
			let (rate, channels) = (source.sample_rate().get(), source.channels().get());
			let seconds = |samples: usize| samples as f64 / f64::from(rate * u32::from(channels));
			// The peak too: past 1.0 on a lossy file shows its decoder no longer flattens at full
			// scale (PLAN Q73), which no generated fixture can.
			let (count, peak) = source.fold((0, 0.0f32), |(count, peak), sample| {
				(count + 1, peak.max(sample.abs()))
			});
			let decoded = seconds(count);
			let header = duration(&path);
			eprintln!(
				"{}: header {header:?}, decoded {decoded:.3} s, peak {peak:.3}, {rate} Hz × {channels}",
				path.display(),
			);
			// No length is no claim: the footer shows a `+`, and there is nothing to disagree with.
			if let Some(header) = header
				&& (header.as_secs_f64() - decoded).abs() > TOLERANCE
			{
				wrong.push(format!("{}: decodes to {decoded:.3} s", path.display()));
			}

			// Just after the start and just before the end too, where a late start, a priming
			// edit and a picture track's edit bite.
			for target in [0.005, decoded / 4.0, decoded * 3.0 / 4.0, decoded - 0.05] {
				let mut sought = decoder(&path).expect("opening the file");
				let result = sought.try_seek(Duration::from_secs_f64(target));
				let landed = decoded - seconds(sought.count());
				eprintln!("  seek to {target:.3} s: {result:?}, landed at {landed:.3} s");
				if result.is_err() || (landed - target).abs() > TOLERANCE {
					wrong.push(format!("{}: a seek to {target:.3} s", path.display()));
				}
			}

			// A seek after one to the very end, which can find nothing left to decode, must leave
			// the decoder where a fresh one would be: rodio swapped left and right there (Q72).
			let quarter = Duration::from_secs_f64(decoded / 4.0);
			let mut fresh = decoder(&path).expect("opening the file");
			let _ = fresh.try_seek(quarter);
			let mut reused = decoder(&path).expect("opening the file");
			let _ = reused.try_seek(Duration::from_secs_f64(decoded));
			let _ = reused.try_seek(quarter);
			// A quarter second runs past the packet a seek lands in, the longest of which, Opus's
			// 120 ms, is half of it. Equal to within -60 dB of the sound's own peak, not exactly:
			// symphonia's AAC decoder keeps its noise generator across a reset, so after
			// another seek its noise never matches a fresh decoder's again (1.7e-5 on a tone at
			// full scale). A swap or a misplaced decode is off by the sound itself, however
			// quiet it is there.
			let quarter_second = (rate / 4) as usize * usize::from(channels);
			let fresh: Vec<f32> = fresh.take(quarter_second).collect();
			let reused: Vec<f32> = reused.take(quarter_second).collect();
			let peak = fresh
				.iter()
				.fold(0.0f32, |peak, sample| peak.max(sample.abs()));
			if fresh.len() != reused.len()
				|| fresh
					.iter()
					.zip(&reused)
					.any(|(a, b)| (a - b).abs() > peak * 1e-3)
			{
				wrong.push(format!("{}: a seek after one to the end", path.display()));
			}

			let scanned = scan(&path).expect("scanning the file");
			eprintln!(
				"  trim {:?}, tempo {:?}, key {:?}",
				scanned.trim, scanned.tempo, scanned.key
			);
		}
		assert!(wrong.is_empty(), "disagreements:\n{}", wrong.join("\n"));
	}

	/// The rate of every MP4 fixture's sound.
	const MP4_RATE: u32 = 8_000;

	/// What `video_first_mp4` builds. By default: one mono frame a packet, a clock that
	/// ticks once a frame, and no edits.
	#[derive(Clone, Copy)]
	struct Sound {
		/// The sound's edit list: one edit playing the media from the tick it names for the
		/// milliseconds it names.
		edit: Option<(u32, u32)>,
		/// The same for the picture's track, whose clock is in milliseconds.
		picture_edit: Option<(u32, u32)>,
		/// The picture's frames, 40 ms each: 25 is the second the sound lasts.
		picture_frames: u32,
		/// The sound clock's ticks a second, which an MP4 need not make its rate. A packet's
		/// length is rounded down to whole ticks, as a muxer may when the two do not divide.
		scale: u32,
		channels: u16,
		/// Frames a packet: AAC's hold 1 024.
		frames: u32,
	}

	impl Default for Sound {
		fn default() -> Self {
			Sound {
				edit: None,
				picture_edit: None,
				picture_frames: 25,
				scale: MP4_RATE,
				channels: 1,
				frames: 1,
			}
		}
	}

	/// One second of MP4 with the video first: a 1 kHz track of 25 opaque frames, then 8 kHz
	/// PCM whose frame `n` is `n`, and `-n` on the right when there is a right. Built by hand
	/// for the same reason `wav` is — it is some twenty kinds of box, each a header and a few
	/// numbers. `avc1` with no body is enough of a picture: symphonia decodes no video, so the
	/// track only has to exist and be timed.
	fn video_first_mp4(sound: Sound) -> Vec<u8> {
		const VIDEO_SCALE: u32 = 1_000;
		let video_frames = sound.picture_frames; // 40 ticks each, at 1 kHz
		const VIDEO_FRAME_BYTES: u32 = 4;

		let video_data = vec![0x7f; (video_frames * VIDEO_FRAME_BYTES) as usize];
		let audio_data: Vec<u8> = (0..MP4_RATE as i16)
			.flat_map(|n| [n, -n].into_iter().take(sound.channels.into()))
			.flat_map(i16::to_le_bytes)
			.collect();

		let pcm_entry = lpcm_entry(sound.channels, sound.frames);
		let tracks = |video_offset: u32, audio_offset: u32| {
			[
				mp4_track(
					1,
					b"vide",
					VIDEO_SCALE,
					mp4_atom(b"avc1", &[]),
					video_frames,
					40,
					VIDEO_FRAME_BYTES,
					video_offset,
					sound.picture_edit,
				),
				mp4_track(
					2,
					b"soun",
					sound.scale,
					pcm_entry.clone(),
					MP4_RATE / sound.frames,
					sound.frames * sound.scale / MP4_RATE,
					sound.frames * u32::from(sound.channels) * 2,
					audio_offset,
					sound.edit,
				),
			]
			.concat()
		};
		let ftyp = mp4_atom(b"ftyp", &[b"isom", &512u32.to_be_bytes(), b"isom"]);
		let mvhd = mp4_movie_header(VIDEO_SCALE);

		// The chunk offsets are absolute, and the moov box before the data is the same size
		// whatever they are, so measure it once with zeros and build it again for real.
		let data_start = (ftyp.len() + mp4_atom(b"moov", &[&mvhd, &tracks(0, 0)]).len() + 8) as u32;
		let moov = mp4_atom(
			b"moov",
			&[
				&mvhd,
				&tracks(data_start, data_start + video_data.len() as u32),
			],
		);

		[ftyp, moov, mp4_atom(b"mdat", &[&video_data, &audio_data])].concat()
	}

	/// One second of the same 8 kHz mono PCM, fragmented, as DASH, OBS and stream captures
	/// write MP4: the moov box lists no samples, and two fragments follow, each a moof box
	/// listing 4 000 one-frame samples and the mdat box holding them.
	fn fragmented_mp4() -> Vec<u8> {
		const FRAGMENT: u32 = MP4_RATE / 2;
		let full = [0; 4]; // a full box's version 0 and no flags
		let one = 1u32.to_be_bytes();

		// The fragments' samples take their length and size from here: one tick, two bytes.
		let trex = mp4_atom(
			b"trex",
			&[&full, &one, &one, &one, &2u32.to_be_bytes(), &[0; 4]],
		);
		let moov = mp4_atom(
			b"moov",
			&[
				&mp4_movie_header(0),
				&mp4_track(1, b"soun", MP4_RATE, lpcm_entry(1, 1), 0, 1, 2, 0, None),
				&mp4_atom(b"mvex", &[&trex]),
			],
		);
		let fragments = (0..2u32).flat_map(|index| {
			// The run's data starts this far from the moof box's start: just past it and the
			// mdat header. The moof box is the same size whatever the offset is.
			let moof = |data_offset: u32| {
				mp4_atom(
					b"moof",
					&[
						&mp4_atom(b"mfhd", &[&full, &(index + 1).to_be_bytes()]),
						&mp4_atom(
							b"traf",
							&[
								&mp4_atom(b"tfhd", &[&full, &one]),
								&mp4_atom(
									b"trun",
									&[
										&[0, 0, 0, 1], // a data offset follows the count
										&FRAGMENT.to_be_bytes(),
										&data_offset.to_be_bytes(),
									],
								),
							],
						),
					],
				)
			};
			let moof = moof(moof(0).len() as u32 + 8);
			let samples: Vec<u8> = (index * FRAGMENT..(index + 1) * FRAGMENT)
				.flat_map(|n| (n as i16).to_le_bytes())
				.collect();
			[moof, mp4_atom(b"mdat", &[&samples])].concat()
		});

		[
			mp4_atom(b"ftyp", &[b"isom", &512u32.to_be_bytes(), b"isom"]),
			moov,
		]
		.into_iter()
		.flatten()
		.chain(fragments)
		.collect()
	}

	/// The movie's header: its clock in milliseconds, and the length given.
	fn mp4_movie_header(duration: u32) -> Vec<u8> {
		mp4_atom(
			b"mvhd",
			&[
				&[0; 12], // version 0, flags, creation and modification times
				&1_000u32.to_be_bytes(),
				&duration.to_be_bytes(),
				&0x0001_0000u32.to_be_bytes(), // rate 1.0
				&0x0100u16.to_be_bytes(),      // volume 1.0
				&[0; 70],                      // reserved, matrix, pre-defined
				&3u32.to_be_bytes(),           // next track id
			],
		)
	}

	/// A sound's sample entry: version 2 of QuickTime's, the one that says how many frames a
	/// packet holds and how many channels a frame has, as 16-bit little-endian PCM.
	fn lpcm_entry(channels: u16, frames: u32) -> Vec<u8> {
		mp4_atom(
			b"lpcm",
			&[
				&[0; 6],                          // reserved
				&1u16.to_be_bytes(),              // data reference index
				&2u16.to_be_bytes(),              // version 2
				&[0; 6],                          // revision, vendor
				&[0, 3, 0, 16, 0xff, 0xfe, 0, 0], // version 0's fields, at their version 2 values
				&0x0001_0000u32.to_be_bytes(),    // and its rate, 1.0
				&72u32.to_be_bytes(),             // the size of the entry's fixed fields
				&f64::from(MP4_RATE).to_be_bytes(),
				&u32::from(channels).to_be_bytes(),
				&0x7f00_0000u32.to_be_bytes(), // reserved
				&16u32.to_be_bytes(),          // bits a sample
				&4u32.to_be_bytes(),           // signed integers, little-endian
				// Bytes a packet, which symphonia ignores, then frames a packet.
				&(u32::from(channels) * 2 * frames).to_be_bytes(),
				&frames.to_be_bytes(),
			],
		)
	}

	/// One track whose samples all sit in a single chunk at `offset`, all the same size and
	/// all `delta` ticks long; and, given `edit`, an edit list of one edit, from that media
	/// time for that many of the movie's ticks.
	#[expect(
		clippy::too_many_arguments,
		reason = "a test fixture's table of numbers"
	)]
	fn mp4_track(
		id: u32,
		handler: &[u8; 4],
		scale: u32,
		entry: Vec<u8>,
		count: u32,
		delta: u32,
		size: u32,
		offset: u32,
		edit: Option<(u32, u32)>,
	) -> Vec<u8> {
		let one = 1u32.to_be_bytes();
		let full = [0; 4]; // a full box's version 0 and no flags
		let tkhd = mp4_atom(
			b"tkhd",
			&[&[0, 0, 0, 3], &[0; 8], &id.to_be_bytes(), &[0; 68]],
		);
		let mdhd = mp4_atom(
			b"mdhd",
			&[
				&full,
				&[0; 8],
				&scale.to_be_bytes(),
				&(count * delta).to_be_bytes(),
				&[0; 4],
			],
		);
		let hdlr = mp4_atom(b"hdlr", &[&full, &[0; 4], handler, &[0; 13]]);
		// A table of one row, or of none for a track of no samples, as a fragmented file's
		// is: its fragments list them.
		let table = |row: &[&[u8]]| match count {
			0 => [full, [0; 4]].concat(),
			_ => [&full[..], &one, &row.concat()].concat(),
		};
		let stbl = mp4_atom(
			b"stbl",
			&[
				&mp4_atom(b"stsd", &[&full, &one, &entry]),
				&mp4_atom(
					b"stts",
					&[&table(&[&count.to_be_bytes(), &delta.to_be_bytes()])],
				),
				&mp4_atom(b"stsc", &[&table(&[&one, &count.to_be_bytes(), &one])]),
				&mp4_atom(b"stsz", &[&full, &size.to_be_bytes(), &count.to_be_bytes()]),
				&mp4_atom(b"stco", &[&table(&[&offset.to_be_bytes()])]),
			],
		);
		let minf = mp4_atom(b"minf", &[&stbl]);
		let mdia = mp4_atom(b"mdia", &[&mdhd, &hdlr, &minf]);

		let edts = edit.map_or_else(Vec::new, |(media_time, duration)| {
			let rate = 0x0001_0000u32; // 1.0, as a 16.16 fraction
			let entry = [duration, media_time, rate].map(u32::to_be_bytes).concat();
			mp4_atom(b"edts", &[&mp4_atom(b"elst", &[&full, &one, &entry])])
		});

		mp4_atom(b"trak", &[&tkhd, &edts, &mdia])
	}

	/// An ISO-BMFF box: a big-endian length that counts its own eight bytes, then its kind.
	fn mp4_atom(kind: &[u8; 4], parts: &[&[u8]]) -> Vec<u8> {
		let body = parts.concat();
		[&(8 + body.len() as u32).to_be_bytes()[..], &kind[..], &body].concat()
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

	/// The same at 8 kHz, in 32-bit floats, which can hold a sample past full scale.
	fn float_wav(samples: &[f32]) -> Vec<u8> {
		const RATE: u32 = 8_000;
		let data_len = samples.len() as u32 * 4;
		[
			&b"RIFF"[..],
			&(36 + data_len).to_le_bytes(),
			b"WAVEfmt ",
			&16u32.to_le_bytes(), // the size of the fmt chunk that follows
			&3u16.to_le_bytes(),  // 3 = IEEE float
			&1u16.to_le_bytes(),  // one channel
			&RATE.to_le_bytes(),
			&(RATE * 4).to_le_bytes(), // bytes per second
			&4u16.to_le_bytes(),       // block align
			&32u16.to_le_bytes(),      // bits per sample
			b"data",
			&data_len.to_le_bytes(),
			&samples
				.iter()
				.flat_map(|s| s.to_le_bytes())
				.collect::<Vec<u8>>(),
		]
		.concat()
	}
}
