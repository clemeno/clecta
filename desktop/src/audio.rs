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
			shared: [Arc::new(Shared::default()), Arc::new(Shared::default())],
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

	/// The shape of almost every downloaded video: the picture is the first track and the
	/// sound the second (PLAN §3). The decoder plays the sound, so the sound is what has to
	/// be measured — rodio read the length off the picture's frame count instead, and an
	/// MP4 of 3:48 showed 1:06. The fixture's video counts in a different unit from its
	/// audio exactly so that mistake cannot come out right by luck.
	#[test]
	fn a_video_first_mp4_is_as_long_as_its_sound() {
		// Arrange
		let path = std::env::temp_dir().join("clecta-video-first-length.mp4");
		std::fs::write(&path, video_first_mp4()).expect("writing the fixture");

		// Act
		let measured = duration(&path);
		let samples = decoder(&path).expect("opening the fixture").count();
		let _ = std::fs::remove_file(&path);

		// Assert: one second exactly, the audio's own 8 000 frames over its own 8 kHz.
		assert_eq!(
			measured,
			Some(Duration::from_secs(1)),
			"the length came off the video"
		);
		// And every one of those frames is sound: the picture's bytes, handed to the audio
		// decoder, would have come out as samples of their own.
		assert_eq!(samples, 8_000, "the video track was decoded as sound");
	}

	/// The same file, sought. rodio clamped every seek to the wrong length above, and then
	/// placed it on the video track's clock — so on the real file a seek to 2:30 landed at
	/// 1:06, and one to 0:30 a frame early. The fixture's samples count up from zero, so the
	/// first one after the seek says exactly where it landed.
	#[test]
	fn a_seek_in_a_video_first_mp4_lands_where_it_was_aimed() {
		// Arrange
		let path = std::env::temp_dir().join("clecta-video-first-seek.mp4");
		std::fs::write(&path, video_first_mp4()).expect("writing the fixture");
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

	/// A WebM as video sites serve it: the picture first, the sound in Opus. symphonia 0.5
	/// demuxes WebM but has no Opus decoder, so the row was offered and every load failed
	/// (PLAN Q71). One second of Opus is fifty 20 ms packets, and all of it is decoded —
	/// the encoder's 312-sample pre-skip included, so the decode, the container's length and
	/// every seek count from the same zero.
	#[test]
	fn a_video_first_opus_webm_is_as_long_as_its_sound() {
		// Arrange
		let path = std::env::temp_dir().join("clecta-video-first-length.webm");
		std::fs::write(&path, video_first_webm()).expect("writing the fixture");

		// Act
		let measured = duration(&path);
		let samples = decoder(&path).expect("opening the fixture").count();
		let _ = std::fs::remove_file(&path);

		// Assert: the container's one second, and exactly one second of 48 kHz frames.
		assert_eq!(measured, Some(Duration::from_secs(1)), "the length");
		assert_eq!(samples, 50 * 960, "the decoded sound");
	}

	/// The same file, sought: half a second in, the remaining 25 packets decode to exactly
	/// the half second the length says is left.
	#[test]
	fn a_seek_in_an_opus_webm_lands_where_it_was_aimed() {
		// Arrange
		let path = std::env::temp_dir().join("clecta-video-first-seek.webm");
		std::fs::write(&path, video_first_webm()).expect("writing the fixture");
		let mut source = decoder(&path).expect("opening the fixture");

		// Act
		let sought = source.try_seek(Duration::from_millis(500));
		let remaining = source.count();
		let _ = std::fs::remove_file(&path);

		// Assert
		assert!(sought.is_ok(), "the seek failed: {sought:?}");
		assert_eq!(remaining, 25 * 960, "what the seek left");
	}

	/// One second of WebM with the video first: a VP9 track of opaque frames, then mono Opus
	/// made of the three-byte packet every Opus decoder reads as 20 ms of silence. Built by
	/// hand for the same reason `video_first_mp4` is. Each element's size is written in the
	/// eight-byte form, which EBML allows for any size and which keeps the arithmetic out.
	fn video_first_webm() -> Vec<u8> {
		const SILENCE: [u8; 3] = [0xf8, 0xff, 0xfe]; // CELT, fullband, 20 ms, one frame
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
				&webm_element(&[0x44, 0x89], &[&1_000f64.to_be_bytes()]), // one second
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
		let cues = |cluster: u64| {
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
					&[&webm_element(&[0xb3], &[&uint(0)]), &position],
				)],
			)
		};
		let block = |track: u8, at: i16, frame: &[u8]| {
			webm_element(
				&[0xa3],
				&[&[0x80 | track], &at.to_be_bytes(), &[0x80], frame],
			)
		};
		let mut cluster = vec![webm_element(&[0xe7], &[&uint(0)])];
		for at in (0..1_000).step_by(20) {
			if at % 40 == 0 {
				cluster.push(block(1, at, &[0; 4]));
			}
			cluster.push(block(2, at, &SILENCE));
		}
		let cluster = webm_element(&[0x1f, 0x43, 0xb6, 0x75], &[&cluster.concat()]);

		// The cue points at the cluster from the start of the segment's body, and the cues
		// before the cluster are the same size whatever that number is.
		let before = info.len() + tracks.len() + cues(0).len();
		let segment = webm_element(
			&[0x18, 0x53, 0x80, 0x67],
			&[&info, &tracks, &cues(before as u64), &cluster],
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

	/// Not a check but a probe, for files the suite cannot ship: what the app learns about
	/// each file named in `CLECTA_PROBE` (a path list, `;`-separated on Windows and
	/// `:`-separated elsewhere) — the length from the header and from a full decode, where
	/// two seeks land, and the scan's answers — so two files that should agree can be read
	/// side by side. It found Q69: an MP4 whose header said 66 s and whose decode said 228.
	///
	/// `CLECTA_PROBE="a.mp3;a.mp4" cargo test probe_files -- --ignored --nocapture`
	#[test]
	#[ignore = "reads the files named in CLECTA_PROBE"]
	fn probe_files() {
		let paths = std::env::var_os("CLECTA_PROBE").expect("CLECTA_PROBE names no files");
		for path in std::env::split_paths(&paths) {
			let source = decoder(&path).expect("opening the file");
			let (rate, channels) = (source.sample_rate().get(), source.channels().get());
			let seconds = |samples: usize| samples as f64 / f64::from(rate * u32::from(channels));
			let decoded = seconds(source.count());
			eprintln!(
				"{}: header {:?}, decoded {decoded:.3} s, {rate} Hz × {channels}",
				path.display(),
				duration(&path),
			);

			for target in [decoded / 4.0, decoded * 3.0 / 4.0] {
				let mut sought = decoder(&path).expect("opening the file");
				let result = sought.try_seek(Duration::from_secs_f64(target));
				let landed = decoded - seconds(sought.count());
				eprintln!("  seek to {target:.3} s: {result:?}, landed at {landed:.3} s");
			}

			let scanned = scan(&path).expect("scanning the file");
			eprintln!(
				"  trim {:?}, tempo {:?}, key {:?}",
				scanned.trim, scanned.tempo, scanned.key
			);
		}
	}

	/// One second of MP4 with the video first: a 1 kHz track of 25 opaque frames, then 8 kHz
	/// mono PCM whose sample `n` is `n`. Built by hand for the same reason `wav` is — it is
	/// eleven kinds of box, each a header and a few numbers. `avc1` with no body is enough of
	/// a picture: symphonia decodes no video, so the track only has to exist and be timed.
	fn video_first_mp4() -> Vec<u8> {
		const RATE: u32 = 8_000;
		const VIDEO_SCALE: u32 = 1_000;
		const VIDEO_FRAMES: u32 = 25; // 40 ticks each: one second at 1 kHz
		const VIDEO_FRAME_BYTES: u32 = 4;

		let video_data = vec![0x7f; (VIDEO_FRAMES * VIDEO_FRAME_BYTES) as usize];
		let audio_data: Vec<u8> = (0..RATE as i16).flat_map(i16::to_le_bytes).collect();

		let pcm_entry = mp4_atom(
			b"sowt",
			&[
				&[0; 6],                     // reserved
				&1u16.to_be_bytes(),         // data reference index
				&[0; 8],                     // version 0, revision, vendor
				&1u16.to_be_bytes(),         // one channel
				&16u16.to_be_bytes(),        // bits per sample
				&[0; 4],                     // compression id, packet size
				&(RATE << 16).to_be_bytes(), // 16.16 fixed point
			],
		);
		let tracks = |video_offset: u32, audio_offset: u32| {
			[
				mp4_track(
					1,
					b"vide",
					VIDEO_SCALE,
					mp4_atom(b"avc1", &[]),
					VIDEO_FRAMES,
					40,
					VIDEO_FRAME_BYTES,
					video_offset,
				),
				mp4_track(
					2,
					b"soun",
					RATE,
					pcm_entry.clone(),
					RATE,
					1,
					2,
					audio_offset,
				),
			]
			.concat()
		};
		let ftyp = mp4_atom(b"ftyp", &[b"isom", &512u32.to_be_bytes(), b"isom"]);
		let mvhd = mp4_atom(
			b"mvhd",
			&[
				&[0; 12], // version 0, flags, creation and modification times
				&VIDEO_SCALE.to_be_bytes(),
				&VIDEO_SCALE.to_be_bytes(),    // one second
				&0x0001_0000u32.to_be_bytes(), // rate 1.0
				&0x0100u16.to_be_bytes(),      // volume 1.0
				&[0; 70],                      // reserved, matrix, pre-defined
				&3u32.to_be_bytes(),           // next track id
			],
		);

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

	/// One track whose samples all sit in a single chunk at `offset`, all the same size and
	/// all `delta` ticks long.
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
		let stbl = mp4_atom(
			b"stbl",
			&[
				&mp4_atom(b"stsd", &[&full, &one, &entry]),
				&mp4_atom(
					b"stts",
					&[&full, &one, &count.to_be_bytes(), &delta.to_be_bytes()],
				),
				&mp4_atom(b"stsc", &[&full, &one, &one, &count.to_be_bytes(), &one]),
				&mp4_atom(b"stsz", &[&full, &size.to_be_bytes(), &count.to_be_bytes()]),
				&mp4_atom(b"stco", &[&full, &one, &offset.to_be_bytes()]),
			],
		);
		let minf = mp4_atom(b"minf", &[&stbl]);
		let mdia = mp4_atom(b"mdia", &[&mdhd, &hdlr, &minf]);

		mp4_atom(b"trak", &[&tkhd, &mdia])
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
}
