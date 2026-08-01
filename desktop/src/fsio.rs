//! The only module that touches the filesystem (PLAN §9).
//!
//! Everything here is blocking and is called from a `Task`, never from `view`. The
//! functions return `Result<_, String>` rather than `anyhow::Error` because the result
//! travels inside a `Message`, which has to be `Clone` — and a message is the wrong place
//! for a backtrace anyway: what reaches the user is one notice line.

use std::path::{Path, PathBuf};

use crate::browser::Entry;

/// Where the tree starts. One root on Unix; on Windows there is no single filesystem
/// root, so the tree gets one per drive letter (PLAN §9).
#[cfg(not(windows))]
pub fn roots() -> Vec<PathBuf> {
	vec![PathBuf::from("/")]
}

/// `ponytail:` probing A: to Z: with `is_dir`, rather than `GetLogicalDrives`. It is one
/// loop against a Win32 call plus a `windows-sys` dependency, and 26 stat calls at
/// startup cost nothing measurable. Swap it if a mapped-but-disconnected network drive
/// ever makes the probe hang.
#[cfg(windows)]
pub fn roots() -> Vec<PathBuf> {
	('A'..='Z')
		.map(|letter| PathBuf::from(format!("{letter}:\\")))
		.filter(|root| root.is_dir())
		.collect()
}

/// The user's home folder, used as the opening view so the browser is not empty on first
/// run.
///
/// `ponytail:` the environment variable, not a `dirs` crate or `std::env::home_dir`. Two
/// names cover both targets, and this is the same "plain std" rule `paths.rs` follows
/// (PLAN §3). Falls back to a root, which is always listable.
pub fn home() -> PathBuf {
	std::env::var_os("HOME")
		.or_else(|| std::env::var_os("USERPROFILE"))
		.map(PathBuf::from)
		.filter(|home| home.is_dir())
		.unwrap_or_else(|| roots().first().cloned().unwrap_or_default())
}

/// List the *files* in one folder. Folders are the tree's job, so they are skipped here
/// (PLAN §9).
///
/// Read whole, not batched: `read_dir` on a local disk returns in milliseconds where
/// cmote's SFTP round trips did not.
pub fn list_files(folder: &Path) -> Result<Vec<Entry>, String> {
	let mut entries = Vec::new();

	for item in read_dir(folder)? {
		// An entry that cannot be read is skipped rather than failing the whole listing:
		// one unreadable file must not hide the other four hundred.
		let Ok(metadata) = item.metadata() else {
			continue;
		};
		if metadata.is_dir() {
			continue;
		}

		entries.push(Entry::new(
			item.path(),
			metadata.len(),
			metadata.modified().ok(),
		));
	}

	Ok(entries)
}

/// List the *subfolders* of one folder, sorted, for the tree.
pub fn list_folders(folder: &Path) -> Result<Vec<PathBuf>, String> {
	let mut folders: Vec<PathBuf> = read_dir(folder)?
		.filter(|item| item.file_type().is_ok_and(|kind| kind.is_dir()))
		.map(|item| item.path())
		.collect();

	folders.sort_by(|a, b| crate::browser::natural_cmp(&name_of(a), &name_of(b)));
	Ok(folders)
}

/// Read a directory, turning both the open error and any per-entry error into something
/// a notice line can show.
fn read_dir(folder: &Path) -> Result<impl Iterator<Item = std::fs::DirEntry>, String> {
	let entries = std::fs::read_dir(folder)
		.map_err(|error| format!("cannot read {}: {error}", folder.display()))?;

	Ok(entries.filter_map(Result::ok))
}

fn name_of(path: &Path) -> String {
	path.file_name()
		.map(|name| name.to_string_lossy().into_owned())
		.unwrap_or_default()
}
