//! Measuring a file: its numbers from the store when the store has them, from the file when it
//! does not, and into the store on the way past (PLAN §11a, §14c). Every scan comes through
//! here — a player's own, a folder scan's, a queue measurement — whoever asked for the decode.
//!
//! These were three free functions at the bottom of `app.rs` with no test, and the rules they
//! hold are the cache's whole contract with the files: no stamp, no caching; a failed scan is
//! not stored; "no length" is (Q77). They take the store as an argument, so the tests run them
//! against a real one in a temporary folder.

use std::path::Path;
use std::time::Duration;

use crate::audio;
use crate::cache::{self, Cache};
use crate::known::Facts;
use crate::waveform::Scan;

/// A track's waveform and the edges of its music.
///
/// The two halves of the stamp rule live here and nowhere else. **No stamp, no caching** — a
/// file that cannot be stat'd is scanned every time, which is the behaviour there was before
/// the cache and is right for a path that is behaving strangely. And a scan that *fails* is
/// not stored: the cache holds answers about files, not the fact that one of them would not
/// open, which is a condition that can change under it.
///
/// What *counts* as a hit is not decided here (Q44). This function used to chain three
/// lookups and know that all three had to answer, which is a rule the store was also enforcing
/// on its own for the files pane's marks — two copies with a comment between them saying they
/// had to be kept in step by hand. Now it asks the store whether the file has been scanned, and
/// the store knows what that means.
pub fn scan(cache: &Cache, path: &Path) -> anyhow::Result<Scan> {
	let stamp = cache::stamp(path);

	if let Some(stamp) = stamp
		&& let Some(scan) = cache.scan(path, stamp)
	{
		return Ok(scan);
	}

	let scan = audio::scan(path)?;
	if let Some(stamp) = stamp {
		cache.store_scan(path, stamp, &scan);
	}
	Ok(scan)
}

/// What is already known about a queued track, without decoding it (PLAN §7a, §14c).
///
/// The trim and the tempo are **read, never worked out**: finding either means decoding the whole
/// file, and a queue edit that decoded fifty tracks would freeze four threads for half a minute
/// for a setting the user may not even have switched on. So a queue learns where a track's music
/// starts and how fast it beats only if something has already scanned it — the folder scan, or the
/// track's own turn in a player — and shows neither until then.
///
/// It asks the same question the files pane's marks ask, through the same function, so the two
/// panes agree about one file (Q44). They did not quite before: this read the trim and the
/// tempo table by table, so a track scanned by a build that knew nothing about tempi showed a
/// playing time in a queue while its row in the pane showed no `✓` at all.
pub fn facts(cache: &Cache, path: &Path) -> Facts {
	Facts {
		duration: duration(cache, path),
		ready: cache::stamp(path).and_then(|stamp| cache.ready(path, stamp)),
		// This job never *prepares* anything — it reads what is there. Saying so here is what
		// stops a queue edit taking marks off rows the store is perfectly happy about.
		prepared: false,
		path: path.to_path_buf(),
	}
}

/// The same, for a track's length (PLAN §7a, §11a).
///
/// The one difference from the waveform is what happens to a failure, and it is not an
/// inconsistency: `audio::duration` has no error to report, only the *absence* of a length,
/// and "this file has no length" is an answer worth remembering — it is exactly what stops the
/// queues re-opening an unreadable file on every edit for the rest of the run.
pub fn duration(cache: &Cache, path: &Path) -> Option<Duration> {
	let Some(stamp) = cache::stamp(path) else {
		return audio::duration(path);
	};

	if let Some(length) = cache.duration(path, stamp) {
		return length;
	}

	let length = audio::duration(path);
	cache.store_duration(path, stamp, length);
	length
}

#[cfg(test)]
mod tests {
	use super::*;
	use std::path::PathBuf;

	/// A folder of its own for one test, emptied first, and a store inside it. Tests run in
	/// parallel, so each gets its own name rather than sharing one.
	fn store(name: &str) -> (PathBuf, Cache) {
		let dir = std::env::temp_dir().join(format!("clecta-measure-{name}"));
		let _ = std::fs::remove_dir_all(&dir);
		std::fs::create_dir_all(&dir).expect("making the test folder");
		let cache = Cache::open(&dir.join("cache.redb"));
		(dir, cache)
	}

	/// Half a second of a quiet tone, as a file a decoder will read.
	fn tone(dir: &Path) -> PathBuf {
		let samples: Vec<i16> = (0..4_000).map(|n| ((n % 40) as i16 - 20) * 100).collect();
		let path = dir.join("tone.wav");
		std::fs::write(&path, audio::tests::wav(&samples, 8_000)).expect("writing the fixture");
		path
	}

	/// A file no decoder will read.
	fn junk(dir: &Path) -> PathBuf {
		let path = dir.join("junk.mp3");
		std::fs::write(&path, b"not audio at all").expect("writing the fixture");
		path
	}

	#[test]
	fn a_scan_is_stored_and_a_second_one_reads_the_store() {
		// Arrange
		let (dir, cache) = store("second-scan");
		let path = tone(&dir);
		let first = scan(&cache, &path).expect("a tone decodes");
		let stamp = cache::stamp(&path).expect("stat of a file just written");

		// Act: the store's answer replaced by one no decode of this file could give.
		let marked = Scan {
			peaks: vec![0.125],
			..first.clone()
		};
		cache.store_scan(&path, stamp, &marked);
		let second = scan(&cache, &path).expect("the store answers");

		// Assert
		assert_eq!(
			cache.scan(&path, stamp).map(|stored| stored.peaks),
			Some(vec![0.125])
		);
		assert_eq!(
			second.peaks,
			vec![0.125],
			"read from the store, not decoded again"
		);
		assert_ne!(first.peaks, second.peaks);
	}

	#[test]
	fn a_scan_that_fails_stores_nothing() {
		// Arrange
		let (dir, cache) = store("failed-scan");
		let path = junk(&dir);

		// Act
		let result = scan(&cache, &path);

		// Assert
		assert!(result.is_err());
		let stamp = cache::stamp(&path).expect("stat of a file just written");
		assert_eq!(cache.scan(&path, stamp), None);
	}

	#[test]
	fn no_length_is_an_answer_worth_remembering() {
		// Arrange
		let (dir, cache) = store("no-length");
		let path = junk(&dir);

		// Act
		let length = duration(&cache, &path);

		// Assert: the store now says "none", which stops a queue reopening it on every edit.
		assert_eq!(length, None);
		let stamp = cache::stamp(&path).expect("stat of a file just written");
		assert_eq!(cache.duration(&path, stamp), Some(None));
	}

	#[test]
	fn a_queue_measurement_reads_a_scan_and_never_prepares() {
		// Arrange: one file nothing has scanned yet.
		let (dir, cache) = store("queue-facts");
		let path = tone(&dir);

		// Act: measured before and after a scan of it.
		let before = facts(&cache, &path);
		scan(&cache, &path).expect("a tone decodes");
		let after = facts(&cache, &path);

		// Assert: a length either way; the scan's numbers only once something scanned it; and
		// never a claim to have prepared it, which would take marks off the pane's rows.
		assert!(before.duration.is_some());
		assert!(before.ready.is_none(), "read, never worked out");
		assert!(after.ready.is_some());
		assert!(!before.prepared && !after.prepared);
	}
}
