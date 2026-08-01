//! The rodio wiring (PLAN §4, §7): one device sink, its mixer, one `rodio::Player` per
//! deck.
//!
//! This is the only module that knows rodio exists. Everything it exposes is either a
//! command to the audio thread or a poll of it — there is no channel back, because rodio
//! does not offer one (PLAN §4).
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
		let file = File::open(path).with_context(|| format!("cannot open {}", path.display()))?;
		let byte_len = file
			.metadata()
			.with_context(|| format!("cannot stat {}", path.display()))?
			.len();

		// The builder, not `Decoder::new`: `with_byte_len` is what makes the stream
		// seekable AND what lets `total_duration()` answer at all (PLAN §7).
		let source = Decoder::builder()
			.with_data(file)
			.with_byte_len(byte_len)
			.with_seekable(true)
			.build()
			.with_context(|| format!("cannot decode {}", path.display()))?;

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
