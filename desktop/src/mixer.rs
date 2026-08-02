//! Pure gain math (PLAN §8): the two volume faders and the crossfader collapsed into
//! the one linear gain each player is set to.
//!
//! No rodio, no iced, no state — which is why this is the module that carries the
//! required test (PLAN §12). The crossfader is arithmetic, not an audio node: rodio's
//! `Player::set_volume` already exists per player, so the crossfade multiplies into it
//! and there is no second gain stage to keep in sync.

use std::f32::consts::FRAC_PI_2;

use serde::{Deserialize, Serialize};

/// The crossfader's shape. Neither curve is right for everything, which is why hardware
/// mixers ship the knob.
///
/// Stored by name in `settings.json`, so the file stays readable and adding a third curve
/// later cannot silently renumber the existing two (PLAN §11).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum Curve {
	/// `g1 = cos(x·π/2)`, `g2 = sin(x·π/2)`. `g1² + g2² = 1` — loudness holds flat.
	/// Right for two DIFFERENT tracks, whose signals are uncorrelated and therefore sum
	/// by power. Both -3 dB at the centre.
	#[default]
	Power,
	/// `g1 = 1-x`, `g2 = x`. `g1 + g2 = 1` — amplitude holds flat. Right for the SAME
	/// beat-matched track on both players, whose signals are correlated and therefore
	/// sum by amplitude. Both -6 dB at the centre.
	Linear,
}

impl Curve {
	/// Both of them, for the selector in the mixer strip.
	pub const ALL: [Curve; 2] = [Curve::Power, Curve::Linear];
}

impl std::fmt::Display for Curve {
	/// What the selector shows. The *reason* the two differ is the interesting part, so
	/// the label says what each is for rather than naming the maths.
	fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
		let label = match self {
			Curve::Power => "Power — different tracks",
			Curve::Linear => "Linear — same track",
		};
		f.write_str(label)
	}
}

/// Collapse both volume faders and the crossfader into the gain each player gets.
///
/// `fader1` / `fader2` are the per-player volume faders (0..=1). `crossfader` is
/// 0 = player 1 alone, 1 = player 2 alone. All three are clamped, so an out-of-range
/// value from a restored settings file cannot produce a nonsense gain.
pub fn gains(fader1: f32, fader2: f32, crossfader: f32, curve: Curve) -> (f32, f32) {
	let x = crossfader.clamp(0.0, 1.0);

	let (cross1, cross2) = match curve {
		Curve::Power => {
			let angle = x * FRAC_PI_2;
			// `cos(π/2)` is -4.4e-8 in f32, not 0. Tiny, but a negative gain is a phase
			// inversion rather than silence, so clamp it away and let the ends be exact.
			(angle.cos().max(0.0), angle.sin().max(0.0))
		}
		Curve::Linear => (1.0 - x, x),
	};

	(taper(fader1) * cross1, taper(fader2) * cross2)
}

/// A volume fader's travel converted to gain.
///
/// Not linear: a linear fader spends its top half on changes the ear barely hears and
/// its bottom inch on everything else. Cubing approximates a dB taper closely enough
/// (fader 0.5 → -18 dB) in one operation, and fader 0 stays exactly silent.
///
/// ponytail: cubic, not a true dB curve. Upgrade path is `10^((fader - 1) * 3)` for a
/// -60 dB..0 dB fader if the feel is wrong.
fn taper(fader: f32) -> f32 {
	let f = fader.clamp(0.0, 1.0);
	f * f * f
}

#[cfg(test)]
mod tests {
	use super::*;

	/// Floating-point comparison at a tolerance far below anything audible.
	fn assert_close(actual: f32, expected: f32, what: &str) {
		assert!(
			(actual - expected).abs() < 1e-6,
			"{what}: expected {expected}, got {actual}"
		);
	}

	#[test]
	fn crossfader_ends_are_exclusive_under_both_curves() {
		for curve in [Curve::Power, Curve::Linear] {
			// Arrange: both faders wide open, so only the crossfade is under test.
			// Act
			let (left1, left2) = gains(1.0, 1.0, 0.0, curve);
			let (right1, right2) = gains(1.0, 1.0, 1.0, curve);

			// Assert
			assert_close(left1, 1.0, "player 1 at crossfader 0");
			assert_close(left2, 0.0, "player 2 at crossfader 0");
			assert_close(right1, 0.0, "player 1 at crossfader 1");
			assert_close(right2, 1.0, "player 2 at crossfader 1");
		}
	}

	#[test]
	fn power_curve_holds_loudness_flat_at_the_centre() {
		// Arrange / Act
		let (g1, g2) = gains(1.0, 1.0, 0.5, Curve::Power);

		// Assert: the defining identity, plus the -3 dB centre it implies.
		assert_close(g1 * g1 + g2 * g2, 1.0, "power identity g1^2 + g2^2");
		assert_close(g1, std::f32::consts::FRAC_1_SQRT_2, "player 1 at centre");
		assert_close(g2, std::f32::consts::FRAC_1_SQRT_2, "player 2 at centre");
	}

	#[test]
	fn linear_curve_holds_amplitude_flat_at_the_centre() {
		// Arrange / Act
		let (g1, g2) = gains(1.0, 1.0, 0.5, Curve::Linear);

		// Assert: the defining identity, plus the -6 dB centre it implies.
		assert_close(g1 + g2, 1.0, "linear identity g1 + g2");
		assert_close(g1, 0.5, "player 1 at centre");
		assert_close(g2, 0.5, "player 2 at centre");
	}

	#[test]
	fn a_fader_at_zero_is_silent_wherever_the_crossfader_sits() {
		for curve in [Curve::Power, Curve::Linear] {
			for step in 0..=10 {
				// Arrange
				let crossfader = step as f32 / 10.0;

				// Act
				let (g1, _) = gains(0.0, 1.0, crossfader, curve);
				let (_, g2) = gains(1.0, 0.0, crossfader, curve);

				// Assert: exactly zero, not merely small — a muted player is muted.
				assert_eq!(g1, 0.0, "player 1 muted at crossfader {crossfader}");
				assert_eq!(g2, 0.0, "player 2 muted at crossfader {crossfader}");
			}
		}
	}

	#[test]
	fn the_fader_taper_is_cubic_and_bounded() {
		// Arrange / Act / Assert: the -18 dB midpoint the taper exists to give.
		assert_close(taper(0.5), 0.125, "fader at half travel");
		assert_close(taper(0.0), 0.0, "fader closed");
		assert_close(taper(1.0), 1.0, "fader wide open");
	}

	#[test]
	fn out_of_range_inputs_are_clamped_rather_than_trusted() {
		// Arrange / Act: what a corrupt settings.json could hand us (PLAN §11). Each
		// out-of-range value must land on the range end it overshot, so the result is
		// bit-identical to passing that end directly.
		let curve = Curve::Power;

		// Assert: the crossfader clamps at both ends...
		assert_eq!(
			gains(1.0, 1.0, -5.0, curve),
			gains(1.0, 1.0, 0.0, curve),
			"crossfader under range"
		);
		assert_eq!(
			gains(1.0, 1.0, 5.0, curve),
			gains(1.0, 1.0, 1.0, curve),
			"crossfader over range"
		);

		// ...and so do the faders, which are the ones that can go negative and would
		// otherwise invert phase rather than mute.
		assert_eq!(
			gains(-5.0, -5.0, 0.5, curve),
			gains(0.0, 0.0, 0.5, curve),
			"faders under range"
		);
		assert_eq!(
			gains(5.0, 5.0, 0.5, curve),
			gains(1.0, 1.0, 0.5, curve),
			"faders over range"
		);
	}
}
