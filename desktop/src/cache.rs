//! The other file clecta writes: `clecta-data/cache.redb` (PLAN §11a).
//!
//! What has already been worked out about a file, so it is not worked out again — a waveform
//! costs a third of a second per track and was being paid on every launch, for the same files.
//!
//! Three rules run through everything here, and they are what make the rest simple:
//!
//! - **It is a cache.** Deleting the file loses nothing but time. Nothing in the app reads
//!   this to decide what is *true*; it only reads it to avoid recomputing what it already
//!   knows. That is why every failure below is swallowed and why a corrupt file is thrown
//!   away rather than repaired.
//! - **A miss is indistinguishable from no cache at all.** Every lookup returns `None` on
//!   anything unexpected — a missing entry, a stale one, a record written by an older format,
//!   a database that would not open. The caller then does what it did before this file
//!   existed.
//! - **Nothing here runs on the GUI thread.** A commit is an `fsync`. Every caller reaches
//!   this from inside an `off_thread` job, which is also why `Cache` is `Sync` and shared as
//!   an `Arc`.

use std::path::Path;
use std::time::{Duration, UNIX_EPOCH};

use redb::{Database, ReadableDatabase, ReadableTableMetadata, TableDefinition};

/// One table per kind of fact, keyed by the same file. Separate rather than one record with
/// optional fields because they are worked out at different moments by different jobs — a
/// waveform when a track is loaded, a length when it is queued — and a table each means a
/// write touches only what it knows. It is also where the *next* kind of fact goes: a table,
/// not a migration.
const WAVEFORMS: TableDefinition<&str, &[u8]> = TableDefinition::new("waveforms");
const DURATIONS: TableDefinition<&str, &[u8]> = TableDefinition::new("durations");

/// The record layout's version, in the first byte of every value.
///
/// One byte, and it buys the only migration story a cache needs: change the layout, bump
/// this, and every old record reads as a miss and is overwritten the next time it is wanted.
/// Without it, a changed layout reads old bytes as new ones — which for an array of floats
/// means a waveform of noise rather than an error.
const FORMAT: u8 = 1;

/// Version, then size, then modified time.
const HEADER: usize = 1 + 8 + 8;

/// Which version of a file a record describes: how big it is, and when it was last written.
///
/// **Not a content hash.** One `stat` against reading the whole file, and the two cases it
/// gets wrong cost exactly one re-scan each: a file edited without changing its length inside
/// its filesystem's timestamp granularity (two seconds on FAT32, which a portable install on
/// a USB stick may well be on), and a file renamed or moved, which loses its entry and earns
/// a new one. A cache that is occasionally cold is a cache; a cache that is occasionally
/// *wrong* would be a bug that looks like a corrupt file.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Stamp {
	size: u64,
	/// Nanoseconds since the epoch, or 0 for a time the platform would not answer for.
	modified: u64,
}

/// Read a file's stamp. `None` when it cannot be stat'd, which is also "do not cache this".
pub fn stamp(path: &Path) -> Option<Stamp> {
	let metadata = std::fs::metadata(path).ok()?;

	let modified = metadata
		.modified()
		.ok()
		.and_then(|time| time.duration_since(UNIX_EPOCH).ok())
		.map_or(0, |since| since.as_nanos() as u64);

	Some(Stamp {
		size: metadata.len(),
		modified,
	})
}

/// The store, or nothing at all when there could not be one.
///
/// `None` is a working state, not an error state: the app runs exactly as it did before this
/// file existed, scanning every waveform every launch. That is the whole reason nothing here
/// returns a `Result` — there is no caller that could do anything useful with one.
pub struct Cache {
	db: Option<Database>,
}

impl Cache {
	/// Open the cache, creating it if it is not there.
	///
	/// A file that will not open is **deleted and recreated** rather than repaired. That is
	/// only safe because this is a cache — the worst it costs is the scans it was holding —
	/// and it turns "corrupt once" into "cold once" instead of "no cache for ever".
	pub fn open(path: &Path) -> Self {
		if let Ok(db) = Database::create(path) {
			return Self { db: Some(db) };
		}

		eprintln!(
			"clecta: rebuilding an unreadable cache at {}",
			path.display()
		);
		let _ = std::fs::remove_file(path);

		match Database::create(path) {
			Ok(db) => Self { db: Some(db) },
			// Said once, on the way past. A folder that cannot be written is already the
			// settings file's problem and says so there; this one only costs speed.
			Err(error) => {
				eprintln!("clecta: running without a cache: {error}");
				Self { db: None }
			}
		}
	}

	/// A track's waveform, if this exact version of the file has already been scanned.
	pub fn peaks(&self, path: &Path, stamp: Stamp) -> Option<Vec<f32>> {
		let payload = self.read(WAVEFORMS, path, stamp)?;
		decode_peaks(&payload)
	}

	/// Remember a scan. Silent on failure, which costs the next launch a re-scan.
	pub fn store_peaks(&self, path: &Path, stamp: Stamp, peaks: &[f32]) {
		self.write(WAVEFORMS, path, stamp, &encode_peaks(peaks));
	}

	/// A track's length, if it has already been looked up.
	///
	/// Two layers, matching `playlist::Item::duration` exactly: the outer is *was this in the
	/// cache*, the inner is *did the file have a length*. A file the decoder could not answer
	/// for is worth remembering as much as one it could, or every launch would re-open it.
	pub fn duration(&self, path: &Path, stamp: Stamp) -> Option<Option<Duration>> {
		let payload = self.read(DURATIONS, path, stamp)?;
		decode_duration(&payload)
	}

	pub fn store_duration(&self, path: &Path, stamp: Stamp, length: Option<Duration>) {
		self.write(DURATIONS, path, stamp, &encode_duration(length));
	}

	/// Drop every entry whose file is no longer there, and say how many went.
	///
	/// The whole growth policy, and it needs no number in it: the cache is bounded by the
	/// library it describes. Run once at startup, on a thread of its own, because it is one
	/// `stat` per entry and the answer changes nothing on screen.
	pub fn prune(&self) -> usize {
		let Some(db) = self.db.as_ref() else {
			return 0;
		};
		let Ok(transaction) = db.begin_write() else {
			return 0;
		};

		let mut dropped = 0;
		for definition in [WAVEFORMS, DURATIONS] {
			let Ok(mut table) = transaction.open_table(definition) else {
				continue;
			};

			// `retain` rather than collect-then-remove: redb's own one-pass form, and the keys
			// are borrowed from the table for exactly as long as the closure runs. It reports
			// nothing about what it dropped, so the count is the difference either side.
			let before = table.len().unwrap_or(0);
			if table.retain(|key, _| Path::new(key).is_file()).is_ok() {
				let after = table.len().unwrap_or(before);
				dropped += before.saturating_sub(after) as usize;
			}
		}

		if transaction.commit().is_err() {
			return 0;
		}
		dropped
	}

	/// One lookup, with the staleness test on the way out. Every `?` here is a cache miss and
	/// none of them is an error the caller could act on.
	fn read(
		&self,
		table: TableDefinition<&str, &[u8]>,
		path: &Path,
		stamp: Stamp,
	) -> Option<Vec<u8>> {
		let key = key(path)?;
		let transaction = self.db.as_ref()?.begin_read().ok()?;
		let table = transaction.open_table(table).ok()?;
		let stored = table.get(key).ok()??;

		// Copied out rather than borrowed: the value guard cannot outlive the transaction,
		// and the payload is a few kilobytes.
		payload(stored.value(), stamp).map(<[u8]>::to_vec)
	}

	/// One write, and the same rule: a failure means the next launch does the work again.
	fn write(
		&self,
		table: TableDefinition<&str, &[u8]>,
		path: &Path,
		stamp: Stamp,
		payload: &[u8],
	) {
		let (Some(key), Some(db)) = (key(path), self.db.as_ref()) else {
			return;
		};
		let Ok(transaction) = db.begin_write() else {
			return;
		};

		{
			let Ok(mut table) = transaction.open_table(table) else {
				return;
			};
			if table
				.insert(key, record(stamp, payload).as_slice())
				.is_err()
			{
				return;
			}
		}

		let _ = transaction.commit();
	}
}

/// What names a file in the store.
///
/// `None` for a path that is not UTF-8, which is then simply never cached. Both shipped
/// platforms produce UTF-8 paths for anything a user typed; the alternative is
/// `OsStr::as_encoded_bytes` and an `unsafe` to get back, which is a real cost for a case
/// whose only symptom is a track that re-scans.
fn key(path: &Path) -> Option<&str> {
	path.to_str()
}

/// A stored value: the format byte, the stamp, then whatever the table's payload is.
fn record(stamp: Stamp, payload: &[u8]) -> Vec<u8> {
	let mut out = Vec::with_capacity(HEADER + payload.len());
	out.push(FORMAT);
	out.extend(stamp.size.to_le_bytes());
	out.extend(stamp.modified.to_le_bytes());
	out.extend(payload);
	out
}

/// The payload of a record, if it was written by this format *and* describes this exact
/// version of the file. Anything else is a miss.
fn payload(record: &[u8], stamp: Stamp) -> Option<&[u8]> {
	if record.len() < HEADER || record[0] != FORMAT {
		return None;
	}

	let size = u64::from_le_bytes(record[1..9].try_into().ok()?);
	let modified = u64::from_le_bytes(record[9..17].try_into().ok()?);
	if (size, modified) != (stamp.size, stamp.modified) {
		return None;
	}

	Some(&record[HEADER..])
}

/// Peaks as they are held in memory — little-endian `f32`, four bytes a column.
///
/// Not quantised to a byte a column, though the strip is only a few hundred pixels wide and
/// would never show the difference. Four times the size of nothing is still nothing (a
/// 2048-column array is 8 KB), and keeping the stored array *bit-identical* to a fresh scan
/// means a cached waveform can never be a suspect when something looks wrong.
fn encode_peaks(peaks: &[f32]) -> Vec<u8> {
	peaks.iter().flat_map(|peak| peak.to_le_bytes()).collect()
}

fn decode_peaks(payload: &[u8]) -> Option<Vec<f32>> {
	if payload.is_empty() || !payload.len().is_multiple_of(4) {
		return None;
	}

	Some(
		payload
			.chunks_exact(4)
			.map(|bytes| f32::from_le_bytes(bytes.try_into().expect("chunks_exact gives four")))
			.collect(),
	)
}

/// A length in nanoseconds, or **no bytes at all** for a file that had none.
///
/// The empty payload is the point: it is how "asked, and there is no length" is told apart
/// from "never asked", which is the difference between reading a byte and re-opening a file.
fn encode_duration(length: Option<Duration>) -> Vec<u8> {
	match length {
		Some(length) => (length.as_nanos() as u64).to_le_bytes().to_vec(),
		None => Vec::new(),
	}
}

fn decode_duration(payload: &[u8]) -> Option<Option<Duration>> {
	match payload.len() {
		0 => Some(None),
		8 => {
			let nanos = u64::from_le_bytes(payload.try_into().ok()?);
			Some(Some(Duration::from_nanos(nanos)))
		}
		// A payload that is neither is a record this build does not understand.
		_ => None,
	}
}

/// Two halves, and both matter. The encoding is pure and is checked without a file: a record
/// that survives a round trip, a stamp that does not match reading as a miss, and every way a
/// payload can be malformed. Then one end-to-end pass over a real database in a temporary
/// folder, because "the bytes are right" and "redb gave them back" are different claims.
#[cfg(test)]
mod tests {
	use super::*;

	fn stamp_of(size: u64, modified: u64) -> Stamp {
		Stamp { size, modified }
	}

	#[test]
	fn a_record_survives_a_round_trip() {
		// Arrange
		let stamp = stamp_of(1_234, 5_678);
		let peaks = vec![0.0, 0.5, 1.0, 0.25];

		// Act
		let stored = record(stamp, &encode_peaks(&peaks));
		let read = payload(&stored, stamp).and_then(decode_peaks);

		// Assert: exactly, not approximately — the whole reason the array is stored as `f32`
		// is that a cached waveform is the same array a scan would have produced.
		assert_eq!(read, Some(peaks));
	}

	#[test]
	fn a_file_that_changed_reads_as_a_miss() {
		// Arrange: a record written for one version of a file.
		let written = stamp_of(1_000, 42);
		let stored = record(written, &encode_peaks(&[1.0]));

		// Act / Assert: the same bytes are only good for the same file.
		assert!(payload(&stored, written).is_some(), "unchanged");
		assert!(
			payload(&stored, stamp_of(1_001, 42)).is_none(),
			"a different length"
		);
		assert!(
			payload(&stored, stamp_of(1_000, 43)).is_none(),
			"written again"
		);
	}

	#[test]
	fn a_record_this_build_cannot_read_is_a_miss_rather_than_noise() {
		// Arrange: the same bytes with a different format byte, and a truncated header.
		let stamp = stamp_of(7, 7);
		let mut stored = record(stamp, &encode_peaks(&[1.0, 0.0]));

		// Act / Assert: an older layout must not be read as this one. Without the version
		// byte it would decode as an array of plausible-looking noise.
		stored[0] = FORMAT + 1;
		assert!(payload(&stored, stamp).is_none(), "another format");

		assert!(payload(&[], stamp).is_none(), "nothing at all");
		assert!(payload(&[FORMAT, 0, 0], stamp).is_none(), "half a header");
	}

	#[test]
	fn a_length_and_the_absence_of_one_are_different_answers() {
		// Arrange / Act / Assert: the distinction the whole two-layer `Option` exists for.
		assert_eq!(
			decode_duration(&encode_duration(Some(Duration::from_millis(215_400)))),
			Some(Some(Duration::from_millis(215_400)))
		);
		assert_eq!(
			decode_duration(&encode_duration(None)),
			Some(None),
			"asked, and the file has no length"
		);

		// And a payload of the wrong shape is not either of them.
		assert_eq!(decode_duration(&[1, 2, 3]), None);
	}

	#[test]
	fn a_malformed_waveform_is_not_half_a_waveform() {
		// Arrange / Act / Assert: an array of floats is four bytes a column, and a payload
		// that is not a whole number of them is a record to throw away rather than to
		// truncate — a waveform missing its last column would draw without complaining.
		assert_eq!(decode_peaks(&[0, 0, 128, 63]), Some(vec![1.0]));
		assert_eq!(decode_peaks(&[0, 0, 128]), None, "three bytes");
		assert_eq!(decode_peaks(&[]), None, "no columns at all");
	}

	#[test]
	fn a_real_database_gives_back_what_was_put_in_it() {
		// Arrange: a real file in a real database, because every test above checks bytes and
		// none of them checks that redb was asked the right question.
		let dir = std::env::temp_dir().join("clecta-cache-test");
		let _ = std::fs::remove_dir_all(&dir);
		std::fs::create_dir_all(&dir).expect("making the test folder");

		let track = dir.join("track.wav");
		std::fs::write(&track, b"not really a wav").expect("writing the fixture");
		// Named `first` rather than `stamp`, which is the function that produced it.
		let first = stamp(&track).expect("stat of a file just written");

		let cache = Cache::open(&dir.join("cache.redb"));

		// Act / Assert: a miss before anything is stored, a hit after.
		assert_eq!(cache.peaks(&track, first), None, "nothing stored yet");
		cache.store_peaks(&track, first, &[0.25, 0.75]);
		assert_eq!(cache.peaks(&track, first), Some(vec![0.25, 0.75]));

		cache.store_duration(&track, first, Some(Duration::from_secs(9)));
		assert_eq!(
			cache.duration(&track, first),
			Some(Some(Duration::from_secs(9)))
		);

		// A file that is not there was never cached, and asking is not an error.
		let missing = dir.join("gone.wav");
		assert_eq!(cache.peaks(&missing, first), None);

		// Act / Assert: writing the file again moves its stamp, and the old entry stops
		// answering for it — the staleness rule, end to end rather than in bytes.
		std::fs::write(&track, b"a different length entirely").expect("rewriting the fixture");
		let moved = stamp(&track).expect("stat again");
		assert_ne!(moved, first, "the fixture must actually look different");
		assert_eq!(cache.peaks(&track, moved), None, "stale");

		// Act / Assert: pruning drops what the library no longer has. The track is still
		// there, so its two entries stay and the deleted one's go.
		cache.store_peaks(&track, moved, &[1.0]);
		let orphan = dir.join("deleted.wav");
		std::fs::write(&orphan, b"briefly").expect("writing the second fixture");
		let orphan_stamp = stamp(&orphan).expect("stat of the second fixture");
		cache.store_peaks(&orphan, orphan_stamp, &[0.5]);
		std::fs::remove_file(&orphan).expect("deleting the second fixture");

		assert_eq!(cache.prune(), 1, "one orphan, one entry");
		assert_eq!(cache.peaks(&track, moved), Some(vec![1.0]), "still here");
		assert_eq!(
			cache.peaks(&orphan, orphan_stamp),
			None,
			"gone with its file"
		);

		drop(cache);
		let _ = std::fs::remove_dir_all(&dir);
	}
}
