//! A folder scan in flight — preparing (PLAN §11b, §11c): which files go out to a thread next,
//! which are out now, and how many have come back.
//!
//! Pure bookkeeping, and the app's to drive: it turns each file this hands out into a `Task`,
//! and reports each answer back with `landed`. The counters were three public fields of a struct
//! in `app.rs`, and its two tests did the driver's arithmetic themselves rather than calling it,
//! so a mistake in the driver passed them (Q77). They are private now, and the tests go through
//! the same four calls the app makes.

use std::path::{Path, PathBuf};

/// How many files a folder scan decodes at once (PLAN §11b).
///
/// Four, and the number is the whole trade: a scan is a decode of an entire file, so one
/// thread at a time leaves a folder of two thousand tracks running for ten minutes, and one
/// thread per core leaves nothing for the audio callback while somebody is playing a set.
/// Four is fast enough to be worth pressing and still short of every machine's core count.
pub const SCAN_JOBS: usize = 4;

/// A folder scan in flight.
///
/// `next` and `done` are separate counters because the files go out ahead of the answers:
/// `SCAN_JOBS` of them are always in the air, so the number on screen has to be the one that
/// has come *back*, and the scan is over when the last thread reports rather than when the
/// last file is handed out.
#[derive(Debug)]
pub struct Preparing {
	files: Vec<PathBuf>,
	next: usize,
	done: usize,
	/// *Which* files are out on a thread right now, so the pane can put a spinner on their
	/// rows (PLAN §11c), and so how many — which is what the fan-out is capped against.
	///
	/// A list rather than a range of `files`, because the answers come back out of order: the
	/// four in the air are never the four between `done` and `next`. At most `SCAN_JOBS` long,
	/// which is why a `Vec` and a linear scan are the whole of it.
	busy: Vec<PathBuf>,
}

impl Preparing {
	/// A scan of these files, nothing handed out yet.
	pub fn new(files: Vec<PathBuf>) -> Self {
		Self {
			files,
			next: 0,
			done: 0,
			busy: Vec::new(),
		}
	}

	/// The files to send out now: whatever the fan-out has free, or whatever is left of the
	/// list, whichever is smaller. Each is counted as out from here until its `landed`.
	pub fn hand_out(&mut self) -> Vec<PathBuf> {
		let room = SCAN_JOBS
			.saturating_sub(self.busy.len())
			.min(self.files.len() - self.next);
		let out = self.files[self.next..self.next + room].to_vec();
		self.next += room;
		self.busy.extend(out.iter().cloned());
		out
	}

	/// One file's answer is back. Its row stops spinning here rather than when the next file
	/// goes out, so a file that is slow to decode is the one still turning.
	pub fn landed(&mut self, path: &Path) {
		if let Some(at) = self.busy.iter().position(|busy| busy == path) {
			self.busy.remove(at);
			self.done += 1;
		}
	}

	/// **Stop**: hand nothing more out, and keep waiting for the files already out.
	///
	/// The threads already out are left to finish — they are decoding a file each, and there is
	/// no way to interrupt a decode that does not also mean checking a flag in the sample loop.
	/// Cutting the list down to what has already gone out, rather than dropping the scan where
	/// it stands, is what keeps the counters honest: the answers still coming belong to this
	/// scan and are counted by it. Dropping it would leave them reporting into a scan that no
	/// longer exists — and starting another before they landed would count their files twice.
	pub fn stop(&mut self) {
		self.files.truncate(self.next);
	}

	/// Whether the scan is over — when the last *thread* has reported, not when the last file
	/// went out. That is also what makes `stop` work without a flag: this becomes true as the
	/// last answers land.
	pub fn is_over(&self) -> bool {
		self.next == self.files.len() && self.busy.is_empty()
	}

	/// How many files have come back, out of how many this scan is for.
	pub fn progress(&self) -> (usize, usize) {
		(self.done, self.files.len())
	}

	/// The files out on a thread this moment.
	pub fn busy(&self) -> &[PathBuf] {
		&self.busy
	}
}

#[cfg(test)]
mod tests {
	use super::*;

	fn files(count: usize) -> Vec<PathBuf> {
		(0..count)
			.map(|n| PathBuf::from(format!("{n}.mp3")))
			.collect()
	}

	/// The whole run, one answer at a time, through the calls the app makes (PLAN §11b). The
	/// `Task`s the driver turns these into need a window; the bookkeeping that decides how many
	/// there are does not.
	#[test]
	fn a_folder_scan_hands_out_exactly_what_it_finishes() {
		// Arrange: ten files and a fan-out of four.
		let mut preparing = Preparing::new(files(10));
		let mut handed = Vec::new();

		// Act: hand out what there is room for, take the oldest back, until it is over.
		while !preparing.is_over() {
			handed.extend(preparing.hand_out());
			assert!(preparing.busy().len() <= SCAN_JOBS, "over the fan-out");
			let first = preparing.busy()[0].clone();
			preparing.landed(&first);
		}

		// Assert: every file went out exactly once, and every one came back.
		assert_eq!(handed, files(10), "every file, once, in order");
		assert_eq!(preparing.progress(), (10, 10));
		assert!(
			preparing.busy().is_empty(),
			"no thread left unaccounted for"
		);
	}

	#[test]
	fn the_first_hand_out_fills_the_fan_out() {
		// Arrange
		let mut preparing = Preparing::new(files(10));

		// Act
		let out = preparing.hand_out();
		let again = preparing.hand_out();

		// Assert: four out, and no room for a fifth until one comes back.
		assert_eq!(out.len(), SCAN_JOBS);
		assert!(again.is_empty());
		assert_eq!(preparing.progress(), (0, 10));
	}

	#[test]
	fn stopping_a_scan_still_waits_for_the_files_already_out() {
		// Arrange: four files out on threads, six still to come.
		let mut preparing = Preparing::new(files(10));
		preparing.hand_out();

		// Act
		preparing.stop();

		// Assert: nothing more goes out, and it is not over until the four report.
		assert!(preparing.hand_out().is_empty(), "nothing more goes out");
		assert!(!preparing.is_over(), "four are still decoding");

		// Out of order, which is the case the busy list exists for: the four in the air are
		// never `files[done..next]`, so a row keeps spinning until *its own* file reports
		// (PLAN §11c).
		for (n, left) in [(2, 3), (0, 2), (3, 1), (1, 0)] {
			preparing.landed(Path::new(&format!("{n}.mp3")));
			assert_eq!(preparing.busy().len(), left, "one row stops per answer");
		}
		assert!(preparing.is_over(), "the last one landed");
		assert_eq!(preparing.progress(), (4, 4), "counted once each");
	}

	#[test]
	fn an_answer_for_a_file_not_out_counts_for_nothing() {
		// Arrange
		let mut preparing = Preparing::new(files(2));
		preparing.hand_out();

		// Act: a stray answer, and the same answer twice.
		preparing.landed(Path::new("elsewhere.mp3"));
		preparing.landed(Path::new("0.mp3"));
		preparing.landed(Path::new("0.mp3"));

		// Assert
		assert_eq!(preparing.progress(), (1, 2));
		assert_eq!(preparing.busy(), [PathBuf::from("1.mp3")]);
	}
}
