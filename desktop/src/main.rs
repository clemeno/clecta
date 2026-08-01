//! THROWAWAY audio spike (PLAN §7, §8) — deleted when the real `main.rs` lands.
//!
//! Everything the plan says about rodio was read off documentation rather than a
//! compiler, so this binary exists to make the four load-bearing claims fail loudly and
//! early, before any UI code depends on them:
//!
//! 1. rodio 0.22 renamed `Sink` to `Player`, reached via a device sink's `Mixer`.
//! 2. Seeking AND `total_duration()` need `Decoder::builder().with_byte_len(..)
//!    .with_seekable(true)` — `Decoder::new` gives neither.
//! 3. `total_duration()` must be read BEFORE `append`, which consumes the source.
//! 4. Stop means `try_seek(ZERO)` + `pause()`, not `Player::stop()`, which drops the
//!    queued source and would force a re-decode.
//!
//! No iced on purpose: audio proven headless cannot be blamed on the GUI thread later.
//!
//! Usage: `cargo run -- <file1> <file2>`

mod mixer;

use anyhow::{Context, Result, bail};
use mixer::{Curve, gains};
use rodio::{Decoder, DeviceSinkBuilder, Player, Source};
use std::fs::File;
use std::path::Path;
use std::thread::sleep;
use std::time::Duration;

/// How long each crossfader position is held while sweeping. Slow enough to hear the
/// curve, short enough to sit through.
const SWEEP_STEP: Duration = Duration::from_millis(150);

/// How many positions a crossfader sweep visits between the two extremes.
const SWEEP_POSITIONS: u32 = 20;

fn main() -> Result<()> {
	let args: Vec<String> = std::env::args().skip(1).collect();
	let [path1, path2] = args.as_slice() else {
		// There is deliberately no window: iced is not a dependency yet, so anyone who
		// launched this expecting the app needs telling why nothing appeared.
		bail!(
			"this is the headless audio spike, not the clecta app — it has no window yet.\n\
			 It needs two audio files to load into the two players:\n\
			 \n    cargo run -- <file1> <file2>\n\n\
			 Got {} argument(s).",
			args.len()
		);
	};

	// Claim 1: one device sink owns the cpal callback thread and hands out a Mixer;
	// each player connects to that mixer and is summed into the output (PLAN §4).
	let sink = DeviceSinkBuilder::open_default_sink().context("no usable audio output device")?;
	println!(
		"device: {} Hz, {} channels",
		sink.config().sample_rate(),
		sink.config().channel_count()
	);

	let player1 = Player::connect_new(sink.mixer());
	let player2 = Player::connect_new(sink.mixer());

	// Claims 2 and 3 live in here.
	let duration1 = load(&player1, Path::new(path1))?;
	let duration2 = load(&player2, Path::new(path2))?;
	println!("player 1: {path1} ({})", describe(duration1));
	println!("player 2: {path2} ({})", describe(duration2));

	// Loading is not playing (PLAN §7) — both tracks sit paused at 0 until now.
	player1.play();
	player2.play();

	for curve in [Curve::Power, Curve::Linear] {
		println!("sweeping the crossfader, {curve:?} curve");
		sweep(&player1, &player2, curve);
	}

	// Claim 2, the seek half: a non-seekable decoder fails right here.
	let target = Duration::from_secs(30);
	player1
		.try_seek(target)
		.with_context(|| format!("player 1 cannot seek to {target:?}"))?;
	sleep(SWEEP_STEP);
	println!(
		"player 1 sought to {target:?}, now at {:?}",
		player1.get_pos()
	);

	// Claim 4: stop is a reset, and the track must still be loaded afterwards.
	stop(&player1);
	stop(&player2);
	println!(
		"stopped — player 1 at {:?}, still loaded: {}",
		player1.get_pos(),
		!player1.empty()
	);

	// Proof the reset is real: playing again starts from the top rather than resuming.
	player1.play();
	sleep(Duration::from_secs(1));
	println!("replayed from stop, now at {:?}", player1.get_pos());

	Ok(())
}

/// Load one file into one player, leaving it paused at 0, and return the track's
/// duration when the decoder can work one out.
fn load(player: &Player, path: &Path) -> Result<Option<Duration>> {
	let file = File::open(path).with_context(|| format!("cannot open {}", path.display()))?;
	let byte_len = file
		.metadata()
		.with_context(|| format!("cannot stat {}", path.display()))?
		.len();

	// The builder form, not `Decoder::new`: `with_byte_len` is what makes the stream
	// seekable AND what lets `total_duration()` answer at all (PLAN §7).
	let source = Decoder::builder()
		.with_data(file)
		.with_byte_len(byte_len)
		.with_seekable(true)
		.build()
		.with_context(|| format!("cannot decode {}", path.display()))?;

	// BEFORE the append below — `append` takes the source by value (PLAN §7).
	let duration = source.total_duration();

	player.pause();
	player.append(source);
	Ok(duration)
}

/// Walk the crossfader from player 1 alone to player 2 alone on one curve, setting the
/// gains the pure mixer math produces.
fn sweep(player1: &Player, player2: &Player, curve: Curve) {
	for step in 0..=SWEEP_POSITIONS {
		let crossfader = step as f32 / SWEEP_POSITIONS as f32;
		let (gain1, gain2) = gains(1.0, 1.0, crossfader, curve);
		player1.set_volume(gain1);
		player2.set_volume(gain2);
		sleep(SWEEP_STEP);
	}
}

/// Stop as the transport defines it (PLAN §7): rewind to 0 and pause, keeping the track
/// loaded. `Player::stop()` would drop the queued source and force a re-decode.
///
/// Pause FIRST, then seek. rodio applies control changes on a 5 ms `periodic_access`
/// tick, and `try_seek` blocks until the audio thread has done the seek — so seeking
/// first leaves the callback playing on from 0 until the pause catches up a tick later,
/// and the playhead settles at 5 ms instead of 0. Pausing first makes both land on the
/// same tick. Safe because rodio's `pausable` sits inside `periodic_access`, so the
/// control tick keeps running while paused.
fn stop(player: &Player) {
	player.pause();
	if let Err(error) = player.try_seek(Duration::ZERO) {
		// ponytail: the spike only reports this. The app falls back to re-opening the
		// file and re-appending it, which is correct for streams that cannot seek.
		eprintln!("stop: rewind failed ({error}), position left where it was");
	}
}

/// A track duration rendered for the console, including the case where the decoder
/// cannot determine one.
fn describe(duration: Option<Duration>) -> String {
	match duration {
		Some(duration) => format!("{:.1}s", duration.as_secs_f32()),
		None => "duration unknown".to_string(),
	}
}
