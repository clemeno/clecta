//! What this run has learned about files (PLAN §14c, Q59) — and the one door those answers
//! arrive through, whoever worked them out.
//!
//! Three jobs produce the same kind of answer at different depths: a track's own scan decodes
//! it, a folder scan decodes it without a player, and a queue measurement only reads the
//! store. Their answers land in three places — the trims the players and the handover read,
//! the marks the files pane draws, the rows the queues show — and the fan-out used to be a
//! method on the app that only two of the three producers went through. What holds it
//! together is one rule, stated here and nowhere else: **a job that could not say says
//! nothing**. `None` is never "there is none", so no answer un-learns an older one.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::Duration;

use crate::browser::Browser;
use crate::cache::Ready;
use crate::queues::Queues;
use crate::waveform::Trim;

/// What a background job worked out about one file (PLAN §7a, §11a, §14c).
///
/// One type for two jobs that answer the same question at different depths: measuring a queue
/// *reads* what is known, and a folder scan *works it out*. The arm that receives them does
/// the same thing either way, which is the point — the app does not care which job found out
/// how long a track is or where its music starts.
#[derive(Debug, Clone)]
pub struct Facts {
	pub path: PathBuf,
	pub duration: Option<Duration>,
	/// What a full scan of the file says — the music's edges and its tempo — or `None` when this
	/// job could not say (Q45). Not "there is none": a queue measurement only *reads* the store,
	/// so a track nothing has scanned comes back with nothing here and keeps whatever it had.
	///
	/// One field rather than the two it used to be, because the two can only ever be found
	/// together: they come out of one decode, they are stored under one rule, and a row showing
	/// a playing time with no tempo beside it was never a state anything could produce.
	pub ready: Option<Ready>,
	/// Whether this job left the store holding everything a load would need (PLAN §11c).
	///
	/// The same shape as `ready` and for the same reason: `false` means *this job did not work
	/// it out*, never "and it is not prepared". A queue measurement only reads, so it says
	/// `false` and takes no mark away — the listing's own question is what removes one.
	pub prepared: bool,
}

/// The answers themselves, plus the fan-out that keeps every pane agreeing about one file.
#[derive(Debug, Default)]
pub struct Known {
	/// Where the music sits inside the files this run has been told about (PLAN §14c).
	///
	/// A map rather than a field on `Deck` and another on `queue::Item`, because the same
	/// answer serves a loaded track and a queued one and a track that is neither yet. A miss
	/// is the ordinary state and means *play this whole*: nothing here is required for the app
	/// to work, which is what makes the folder scan an optimization rather than a step.
	trims: HashMap<PathBuf, Trim>,
}

impl Known {
	/// Where a file's music sits, if anything has said (PLAN §14c) — read by the handover's
	/// early cut, the track it starts next, and the buttons above the strips.
	pub fn trim(&self, path: &Path) -> Option<Trim> {
		self.trims.get(path).copied()
	}

	/// Where a file's music sits, if the job that looked found any (PLAN §14c).
	///
	/// A `None` is *not* stored as "there is no trim", and the distinction matters: it means
	/// this job could not say, and another one still might — a queue measurement only reads the
	/// cache, where a folder scan decodes. Overwriting a known answer with silence would make
	/// queueing a track un-learn what scanning it taught.
	pub fn remember(&mut self, path: &Path, trim: Option<Trim>) {
		if let Some(trim) = trim {
			self.trims.insert(path.to_path_buf(), trim);
		}
	}

	/// Record what a job worked out about one file, everywhere the answer shows (Q59): the
	/// trim kept here, the mark on the pane's row, and every queued row holding the file.
	///
	/// The queues are settled by path, because a track can be moved between them while the
	/// answer is being looked up — and one answer settles every row holding that file. A row
	/// removed in the meantime simply matches nothing. Settling them also lets go of the file:
	/// this answer is done with it whatever the queues have done with the rows, and a path
	/// left in flight would never be looked up again.
	pub fn learned(&mut self, facts: &Facts, browser: &mut Browser, queues: &mut Queues) {
		self.remember(&facts.path, facts.ready.and_then(|ready| ready.trim));
		if facts.prepared
			&& let Some(ready) = facts.ready
		{
			browser.mark_prepared(&facts.path, ready);
		}
		queues.measured(&facts.path, facts.duration, facts.ready);
	}
}

#[cfg(test)]
mod tests {
	use super::*;
	use crate::browser::Entry;
	use crate::deck::DeckId;
	use crate::queue::Queue;
	use crate::queues::QueueId;

	fn path() -> PathBuf {
		PathBuf::from("/m/a.mp3")
	}

	/// A pane listing the file and a cue holding it — the two other places an answer lands.
	fn panes() -> (Browser, Queues) {
		let mut browser = Browser::default();
		browser.show(PathBuf::from("/m"), vec![Entry::new(path(), 9, None)]);
		let queues = Queues::restored([
			Queue::from_paths(vec![path()]),
			Queue::default(),
			Queue::default(),
		]);
		(browser, queues)
	}

	#[test]
	fn one_answer_lands_in_every_place_that_shows_it() {
		// Arrange: a folder scan's answer about one file.
		let (mut browser, mut queues) = panes();
		let mut known = Known::default();
		let ready = Ready {
			tempo: Some(128.0),
			trim: Some(Trim {
				start: Duration::from_secs(1),
				end: Duration::from_secs(9),
			}),
		};

		// Act
		known.learned(
			&Facts {
				path: path(),
				duration: Some(Duration::from_secs(10)),
				ready: Some(ready),
				prepared: true,
			},
			&mut browser,
			&mut queues,
		);

		// Assert: the trim for the players, the mark for the pane, the row for the queue —
		// one answer, three homes, and no way for them to disagree about one file.
		assert_eq!(known.trim(&path()), ready.trim);
		assert_eq!(browser.ready(&path()), Some(ready));
		assert_eq!(
			queues.get(QueueId::Cue(DeckId::One)).items()[0].ready,
			Some(ready)
		);
	}

	#[test]
	fn a_job_that_could_not_say_takes_nothing_away() {
		// Arrange: a trim already learned from a scan.
		let (mut browser, mut queues) = panes();
		let mut known = Known::default();
		let trim = Some(Trim {
			start: Duration::from_secs(1),
			end: Duration::from_secs(9),
		});
		known.remember(&path(), trim);

		// Act: a queue measurement that only read the store and found nothing — `ready: None`
		// means "this job could not say", never "there is none" (Q45).
		known.learned(
			&Facts {
				path: path(),
				duration: None,
				ready: None,
				prepared: false,
			},
			&mut browser,
			&mut queues,
		);

		// Assert: the trim stands, and a job that only reads marks nothing prepared.
		assert_eq!(
			known.trim(&path()),
			trim,
			"no answer un-learns an older one"
		);
		assert!(!browser.is_prepared(&path()));
	}
}
