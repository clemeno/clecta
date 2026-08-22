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

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use redb::{Database, ReadableDatabase, ReadableTableMetadata, TableDefinition};

use crate::waveform::{Scan, Trim};

/// One table per kind of fact, keyed by the same file. Separate rather than one record with
/// optional fields because they are worked out at different moments by different jobs — a
/// waveform when a track is loaded, a length when it is queued — and a table each means a
/// write touches only what it knows. It is also where the *next* kind of fact goes: a table,
/// not a migration.
///
/// `trims` is that promise being kept, and it is the interesting case: the music's two edges
/// (PLAN §14c) are worked out by the *same* pass as the waveform, so they could have been two
/// more fields on that record. A table of their own costs one lookup and buys two things —
/// every waveform already on disk stays readable, where a changed layout would have needed
/// `FORMAT` bumped and every one of them rescanned, and a handover that wants a track's start
/// reads sixteen bytes rather than eight kilobytes of amplitudes it has no use for.
const WAVEFORMS: TableDefinition<&str, &[u8]> = TableDefinition::new("waveforms");
const DURATIONS: TableDefinition<&str, &[u8]> = TableDefinition::new("durations");
const TRIMS: TableDefinition<&str, &[u8]> = TableDefinition::new("trims");
/// The promise above kept a second time, and the case that shows the design was right: the tempo
/// (PLAN §14d) is worked out by the same pass again, and again it gets a table rather than a
/// field — so a build that knows about tempi reads every trim already on disk, and the one thing
/// a hand-edited tempo will need (PLAN §14d) is a store of its own it can be cleared from without
/// touching a waveform.
const TEMPOS: TableDefinition<&str, &[u8]> = TableDefinition::new("tempos");

/// All of them, for the two operations that are about the store rather than about a file:
/// pruning what the library no longer has, and emptying it on request.
const TABLES: [TableDefinition<&str, &[u8]>; 4] = [WAVEFORMS, DURATIONS, TRIMS, TEMPOS];

/// The three a full scan is spread across, **in the order a `Scan`'s fields are written**.
///
/// This array is the all-or-nothing rule, and having it in one place is the point: the rule used
/// to be written twice, once in the app's `cached_scan` and once in `prepared` here, with a
/// comment saying the two had to be kept in step by hand. Adding the tempo table meant editing
/// both, which is how a comment like that gets found out.
const FULL: [TableDefinition<&str, &[u8]>; 3] = [WAVEFORMS, TRIMS, TEMPOS];

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
	Some(stamp_of(metadata.len(), metadata.modified().ok()))
}

/// The same stamp, from metadata somebody has already read.
///
/// The files pane's rows carry a size and a modified time because they are *shown* — so asking
/// which of a listing the store already knows costs no `stat` at all (PLAN §11c). It has to be
/// the same two numbers `stamp` uses or the answer would be wrong rather than merely slow,
/// which is why the function above is now written in terms of this one.
pub fn stamp_of(size: u64, modified: Option<SystemTime>) -> Stamp {
	Stamp {
		size,
		modified: modified
			.and_then(|time| time.duration_since(UNIX_EPOCH).ok())
			.map_or(0, |since| since.as_nanos() as u64),
	}
}

/// What one decode of a file worked out, minus the waveform itself (PLAN §11c, §14c, §14d).
///
/// A `Scan` with the eight kilobytes taken off, and the reason to have it separately is that
/// almost nothing wants the eight kilobytes: a files-pane row shows two numbers, a queue row
/// shows two numbers, and a handover wants one `Duration` out of the trim. Only the player
/// panel draws the array.
///
/// One value rather than two maps, because it is one answer read from one set of tables in one
/// pass: a tick with no tempo beside it and a tempo on an unticked row are both states that
/// cannot happen, and the cheapest way to keep them impossible is to leave them unrepresentable.
///
/// Each field keeps its own `None`, which here means *scanned, and there is none* — silence has
/// no music in it and a spoken word recording has no tempo. "Nobody has scanned this" is the
/// absence of the whole value.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct Ready {
	pub tempo: Option<f32>,
	/// The music's two edges, kept rather than the length between them: a handover seeks to
	/// `start` and a strip marks both, so a stored length would have to be a stored trim as well.
	pub trim: Option<Trim>,
}

impl Ready {
	/// How long the music runs, for the one column both panes show.
	///
	/// The only place `Trim::music` is now called. It was written three times across two
	/// modules before this — once here in `prepared`, twice in the app — which for arithmetic
	/// this cheap is not a performance problem but is three chances to disagree.
	pub fn music(self) -> Option<Duration> {
		self.trim.map(Trim::music)
	}
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

	/// Everything one decode of this exact version of the file worked out, or nothing at all.
	///
	/// **All three tables or none of them**, which is the rule that makes a hit here mean the
	/// same thing as a `✓` in the files pane (Q34). A file scanned by a build that knew nothing
	/// about tempi has a waveform and edges on disk and no tempo, so it is a miss and is decoded
	/// once more — which is what lets a new kind of fact arrive without bumping `FORMAT` and
	/// throwing away every waveform in the store to add a field (PLAN §11a).
	///
	/// The caller does not know how many tables there are, what order they are in, or that the
	/// stamp is checked three times. It asks whether the file has been scanned and is told.
	pub fn scan(&self, path: &Path, stamp: Stamp) -> Option<Scan> {
		let [peaks, trim, tempo] = self.full(path, stamp)?;
		Some(Scan {
			peaks: decode_peaks(&peaks)?,
			trim: decode_trim(&trim)?,
			tempo: decode_tempo(&tempo)?,
		})
	}

	/// Remember a scan: every table, in one commit.
	///
	/// One transaction rather than three, which is one `fsync` rather than three and — the part
	/// that matters — makes a half-stored scan impossible. Three separate writes could leave a
	/// waveform on disk with no tempo beside it, which is a state `scan` would read as a miss for
	/// ever and re-store identically every time.
	///
	/// Silent on failure, which costs the next launch a re-scan.
	pub fn store_scan(&self, path: &Path, stamp: Stamp, scan: &Scan) {
		let payloads = [
			encode_peaks(&scan.peaks),
			encode_trim(scan.trim),
			encode_tempo(scan.tempo),
		];

		let (Some(key), Some(db)) = (key(path), self.db.as_ref()) else {
			return;
		};
		let Ok(transaction) = db.begin_write() else {
			return;
		};

		{
			for (definition, payload) in FULL.into_iter().zip(&payloads) {
				let Ok(mut table) = transaction.open_table(definition) else {
					return;
				};
				if table
					.insert(key, record(stamp, payload).as_slice())
					.is_err()
				{
					return;
				}
			}
		}

		let _ = transaction.commit();
	}

	/// What a row shows once the file has been scanned, without reading the waveform back
	/// (PLAN §11c, §14c, §14d).
	///
	/// The same all-or-nothing test `scan` makes — literally the same function — so a row that
	/// shows a playing time is a row a load will not decode. What it skips is turning eight
	/// kilobytes of little-endian bytes back into `f32`s that nothing is going to draw, which is
	/// the whole reason a queue edit reading fifty tracks is cheap.
	pub fn ready(&self, path: &Path, stamp: Stamp) -> Option<Ready> {
		let [_, trim, tempo] = self.full(path, stamp)?;
		Some(Ready {
			tempo: decode_tempo(&tempo)?,
			trim: decode_trim(&trim)?,
		})
	}

	/// A track's length, if it has already been looked up.
	///
	/// Two layers, matching `queue::Item::duration` exactly: the outer is *was this in the
	/// cache*, the inner is *did the file have a length*. A file the decoder could not answer
	/// for is worth remembering as much as one it could, or every launch would re-open it.
	pub fn duration(&self, path: &Path, stamp: Stamp) -> Option<Option<Duration>> {
		let payload = self.read(DURATIONS, path, stamp)?;
		decode_duration(&payload)
	}

	pub fn store_duration(&self, path: &Path, stamp: Stamp, length: Option<Duration>) {
		self.write(DURATIONS, path, stamp, &encode_duration(length));
	}

	/// Which of these files the store already answers for **in full**, and what each of those
	/// answers is (PLAN §11c, §14c, §14d).
	///
	/// One `ready` per file and nothing else: the mark is the key being present, and its value is
	/// everything the row shows beside the mark. Both come out of the same read, and the test is
	/// the one `scan` makes, so a marked row is a row that will not be decoded again.
	///
	/// `ponytail:` an 8 KB copy per file, because the waveform's payload is fetched and then not
	/// decoded. One transaction now rather than three, which was the note that used to be here;
	/// what is left is a header-only presence test, worth writing the day a listing of tens of
	/// thousands takes long enough to see.
	pub fn prepared(&self, files: &[(PathBuf, Stamp)]) -> HashMap<PathBuf, Ready> {
		files
			.iter()
			.filter_map(|(path, stamp)| Some((path.clone(), self.ready(path, *stamp)?)))
			.collect()
	}

	/// Empty the store — every table, every file.
	///
	/// The one destructive thing in this module, and it is safe for the reason the whole file
	/// rests on: this is a cache. What it costs is the scans it was holding, which is time and
	/// nothing else, and that is exactly what the button asking for it means by a clean start.
	///
	/// The tables are dropped rather than walked: redb takes the whole table out in one commit
	/// and recreates it empty on the next write, where a `retain` that kept nothing would still
	/// be one pass over every entry.
	pub fn clear(&self) {
		let Some(db) = self.db.as_ref() else {
			return;
		};
		let Ok(transaction) = db.begin_write() else {
			return;
		};

		for definition in TABLES {
			// A table that was never created is not an error: nothing has been stored in it,
			// which is the state being asked for.
			let _ = transaction.delete_table(definition);
		}

		let _ = transaction.commit();
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
		for definition in TABLES {
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

	/// The three payloads a full scan is stored as, in **one** read transaction — or `None`
	/// unless every one of them answers for this exact version of the file.
	///
	/// The all-or-nothing rule lives here, once, and both public readers go through it. Every `?`
	/// is a miss: a table that was never created, a file with no entry in one of them, a record
	/// from an older `FORMAT`, or a stamp that has moved since it was written. The caller cannot
	/// see which, and there is nothing it would do differently if it could.
	///
	/// The payloads come back undecoded so the one caller that does not need the waveform does
	/// not pay to turn it back into floats.
	fn full(&self, path: &Path, stamp: Stamp) -> Option<[Vec<u8>; 3]> {
		let key = key(path)?;
		let transaction = self.db.as_ref()?.begin_read().ok()?;

		let fetch = |definition| -> Option<Vec<u8>> {
			let table = transaction.open_table(definition).ok()?;
			let stored = table.get(key).ok()??;
			// Copied out rather than borrowed: the value guard cannot outlive the table.
			payload(stored.value(), stamp).map(<[u8]>::to_vec)
		};

		Some([fetch(FULL[0])?, fetch(FULL[1])?, fetch(FULL[2])?])
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

/// Two lengths in nanoseconds, or **no bytes at all** for a file with no music in it — the
/// same shape as a duration, and told apart from "never scanned" the same way.
fn encode_trim(trim: Option<Trim>) -> Vec<u8> {
	match trim {
		Some(trim) => {
			let mut out = Vec::with_capacity(16);
			out.extend((trim.start.as_nanos() as u64).to_le_bytes());
			out.extend((trim.end.as_nanos() as u64).to_le_bytes());
			out
		}
		None => Vec::new(),
	}
}

/// A tempo in beats per minute, or **no bytes at all** for a file that beats at none — the third
/// use of the same shape, and the reason it is a shape rather than three ad-hoc encodings.
///
/// Stored as the `f32` the detector produced rather than as the hundredths the column prints. The
/// rounding belongs to the display: a stored 128.00 could not be told from a stored 128.004, and
/// the day a tempo is halved or doubled by hand (PLAN §14d) is the day that difference starts
/// compounding.
fn encode_tempo(tempo: Option<f32>) -> Vec<u8> {
	match tempo {
		Some(tempo) => tempo.to_le_bytes().to_vec(),
		None => Vec::new(),
	}
}

fn decode_tempo(payload: &[u8]) -> Option<Option<f32>> {
	match payload.len() {
		0 => Some(None),
		4 => {
			let tempo = f32::from_le_bytes(payload.try_into().ok()?);
			// A stored `NaN` or a negative would print as a tempo nobody could act on, and it is
			// cheaper to call that a miss than to teach the column about it.
			(tempo.is_finite() && tempo > 0.0).then_some(Some(tempo))
		}
		_ => None,
	}
}

fn decode_trim(payload: &[u8]) -> Option<Option<Trim>> {
	match payload.len() {
		0 => Some(None),
		16 => {
			let start = u64::from_le_bytes(payload[..8].try_into().ok()?);
			let end = u64::from_le_bytes(payload[8..].try_into().ok()?);
			Some(Some(Trim {
				start: Duration::from_nanos(start),
				end: Duration::from_nanos(end),
			}))
		}
		_ => None,
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

	fn stamped(size: u64, modified: u64) -> Stamp {
		Stamp { size, modified }
	}

	#[test]
	fn a_record_survives_a_round_trip() {
		// Arrange
		let stamp = stamped(1_234, 5_678);
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
		let written = stamped(1_000, 42);
		let stored = record(written, &encode_peaks(&[1.0]));

		// Act / Assert: the same bytes are only good for the same file.
		assert!(payload(&stored, written).is_some(), "unchanged");
		assert!(
			payload(&stored, stamped(1_001, 42)).is_none(),
			"a different length"
		);
		assert!(
			payload(&stored, stamped(1_000, 43)).is_none(),
			"written again"
		);
	}

	#[test]
	fn a_record_this_build_cannot_read_is_a_miss_rather_than_noise() {
		// Arrange: the same bytes with a different format byte, and a truncated header.
		let stamp = stamped(7, 7);
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
	fn a_trim_and_the_absence_of_music_are_different_answers() {
		// Arrange / Act / Assert: the same two-layer rule the durations follow. "Scanned, and
		// there is no music in this file" has to be told from "never scanned", or a track of
		// silence is decoded again on every launch.
		let trim = Trim {
			start: Duration::from_millis(1_200),
			end: Duration::from_millis(214_800),
		};
		assert_eq!(decode_trim(&encode_trim(Some(trim))), Some(Some(trim)));
		assert_eq!(
			decode_trim(&encode_trim(None)),
			Some(None),
			"scanned, and silent throughout"
		);

		// A payload of the wrong shape is neither — including one that is exactly half a
		// trim, which would otherwise read as a start with no end.
		assert_eq!(decode_trim(&[0; 8]), None, "half a trim");
		assert_eq!(decode_trim(&[1, 2, 3]), None);
	}

	#[test]
	fn a_tempo_and_the_absence_of_one_are_different_answers() {
		// Arrange / Act / Assert: the same two-layer rule a third time. "Scanned, and this file
		// beats at nothing" costs a whole decode to find out and is worth a record of its own,
		// or a spoken word file is decoded again on every launch (PLAN §14d).
		assert_eq!(decode_tempo(&encode_tempo(Some(128.5))), Some(Some(128.5)));
		assert_eq!(
			decode_tempo(&encode_tempo(None)),
			Some(None),
			"scanned, and there is no tempo in it"
		);

		// A stored number that could not be printed as a tempo is a miss rather than a column
		// showing it: the record is thrown away and the file is scanned again.
		assert_eq!(decode_tempo(&f32::NAN.to_le_bytes()), None, "not a number");
		assert_eq!(decode_tempo(&(-4.0_f32).to_le_bytes()), None, "negative");
		assert_eq!(decode_tempo(&[1, 2, 3]), None, "the wrong shape");
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
		let scanned = Scan {
			peaks: vec![0.25, 0.75],
			trim: Some(Trim {
				start: Duration::from_millis(500),
				end: Duration::from_secs(9),
			}),
			tempo: Some(128.5),
		};

		// Act / Assert: a miss before anything is stored, and one answer after — all three
		// tables written and read as a unit, which is the whole of the interface now.
		assert_eq!(cache.scan(&track, first), None, "nothing stored yet");
		cache.store_scan(&track, first, &scanned);
		assert_eq!(cache.scan(&track, first), Some(scanned.clone()));

		cache.store_duration(&track, first, Some(Duration::from_secs(9)));
		assert_eq!(
			cache.duration(&track, first),
			Some(Some(Duration::from_secs(9)))
		);

		// A file that is not there was never cached, and asking is not an error.
		let missing = dir.join("gone.wav");
		assert_eq!(cache.scan(&missing, first), None);

		// Act / Assert: writing the file again moves its stamp, and the old entry stops
		// answering for it — the staleness rule, end to end rather than in bytes.
		std::fs::write(&track, b"a different length entirely").expect("rewriting the fixture");
		let moved = stamp(&track).expect("stat again");
		assert_ne!(moved, first, "the fixture must actually look different");
		assert_eq!(cache.scan(&track, moved), None, "stale");

		// Act / Assert: pruning drops what the library no longer has. The track is still
		// there, so its entries stay and the deleted one's go — three of them, one per table a
		// scan is spread across, which is the count that says `store_scan` wrote all three.
		cache.store_scan(&track, moved, &scanned);
		let orphan = dir.join("deleted.wav");
		std::fs::write(&orphan, b"briefly").expect("writing the second fixture");
		let orphan_stamp = stamp(&orphan).expect("stat of the second fixture");
		cache.store_scan(&orphan, orphan_stamp, &scanned);
		std::fs::remove_file(&orphan).expect("deleting the second fixture");

		assert_eq!(cache.prune(), FULL.len(), "one orphan, one entry per table");
		assert_eq!(
			cache.scan(&track, moved),
			Some(scanned.clone()),
			"still here"
		);
		assert_eq!(
			cache.scan(&orphan, orphan_stamp),
			None,
			"gone with its file"
		);

		// Act / Assert: a hit is every table or none of them. A file with a waveform and a tempo
		// and no edges is still a whole decode away, which is exactly what the mark in the files
		// pane promises it is not (PLAN §11c). This is the one state the interface above cannot
		// produce, and it is the state a store written by an older build is *in* — so it is
		// written here through the private door, which is what an older build effectively is.
		let half = dir.join("half.wav");
		std::fs::write(&half, b"only a waveform").expect("writing the third fixture");
		let half_stamp = stamp(&half).expect("stat of the third fixture");
		cache.write(WAVEFORMS, &half, half_stamp, &encode_peaks(&[0.1]));
		cache.write(TEMPOS, &half, half_stamp, &encode_tempo(Some(120.0)));
		assert_eq!(cache.scan(&half, half_stamp), None, "two tables of three");

		// And one scanned and found to hold no music at all, which is an answer rather than a
		// gap: it is marked, and it has neither a playing time nor a tempo to give.
		let silent = dir.join("silent.wav");
		std::fs::write(&silent, b"nothing audible").expect("writing the fourth fixture");
		let silent_stamp = stamp(&silent).expect("stat of the fourth fixture");
		cache.store_scan(
			&silent,
			silent_stamp,
			&Scan {
				peaks: vec![0.0],
				trim: None,
				tempo: None,
			},
		);

		// The answer carries the tempo and the edges with it, both read from what `scan` was
		// reading anyway (PLAN §14c, §14d) — 8.5 s of music in a 9 s file.
		let asked = [
			(track.clone(), moved),
			(half.clone(), half_stamp),
			(silent.clone(), silent_stamp),
		];
		let marked = cache.prepared(&asked);
		assert_eq!(
			marked,
			HashMap::from([
				(track.clone(), cache.ready(&track, moved).expect("scanned")),
				(silent.clone(), Ready::default()),
			]),
			"the half-written one is not marked"
		);
		assert_eq!(marked[&track].tempo, Some(128.5));
		assert_eq!(marked[&track].music(), Some(Duration::from_millis(8_500)));
		assert_eq!(marked[&silent].music(), None, "scanned, and silent");

		// And the listing's own metadata makes the same stamp a `stat` does, which is the whole
		// reason marking a folder costs no filesystem work.
		let listed = std::fs::metadata(&track).expect("stat once more");
		assert_eq!(stamp_of(listed.len(), listed.modified().ok()), moved);

		cache.clear();
		assert!(cache.prepared(&asked).is_empty(), "cleared");
		assert_eq!(cache.scan(&track, moved), None, "cleared");
		assert_eq!(cache.duration(&track, moved), None, "cleared");

		// And an emptied store is still a store: what goes in after comes back out.
		cache.store_scan(&track, moved, &scanned);
		assert_eq!(
			cache.scan(&track, moved),
			Some(scanned),
			"usable after clearing"
		);

		drop(cache);
		let _ = std::fs::remove_dir_all(&dir);
	}
}
