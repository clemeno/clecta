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

/// Every media file in a folder and everything under it, for the folder scan (PLAN §11b).
///
/// The one recursive read in the app, and it is deliberately not what the files pane does:
/// the pane shows one folder because that is where the user is, and this walks the tree
/// because "prepare this folder" means the evening's music, which lives in a folder of
/// albums.
///
/// **Symbolic links to folders are not followed**, which is what makes the walk terminate: a
/// link pointing at its own ancestor is a loop, and `DirEntry::file_type` reports a link as a
/// link rather than as the folder it points at. A link to a *file* is still collected, because
/// that is what the pane already lists and playing one works.
pub fn media_tree(root: &Path) -> Result<Vec<PathBuf>, String> {
	// The root is read here so that an unreadable *root* is reported: it is the folder the
	// user is looking at, and a scan that found nothing there must not look like a folder with
	// nothing in it. Everything deeper is skipped in silence by `collect` — one locked
	// subfolder cancelling a scan of four hundred readable ones would be the wrong trade.
	let mut found = Vec::new();
	collect(read_dir(root)?, &mut found);

	// In the order the pane would show them, folder by folder, so the count that ticks past
	// while the scan runs matches what the eye expects.
	found.sort_by(|a, b| crate::browser::natural_cmp(&a.to_string_lossy(), &b.to_string_lossy()));
	Ok(found)
}

/// One folder's entries, and everything under whichever of them are folders.
fn collect(entries: impl Iterator<Item = std::fs::DirEntry>, found: &mut Vec<PathBuf>) {
	for item in entries {
		let Ok(kind) = item.file_type() else {
			continue;
		};
		let path = item.path();

		if kind.is_dir() {
			if let Ok(children) = read_dir(&path) {
				collect(children, found);
			}
		} else if crate::browser::kind_of(&path).is_media() {
			found.push(path);
		}
	}
}

/// Read a directory, turning both the open error and any per-entry error into something
/// a notice line can show.
fn read_dir(folder: &Path) -> Result<impl Iterator<Item = std::fs::DirEntry>, String> {
	let entries = std::fs::read_dir(folder)
		.map_err(|error| format!("cannot read {}: {error}", folder.display()))?;

	Ok(entries.filter_map(Result::ok))
}

/// A path's last component, for a notice line or a sort key.
///
/// Falls back to the whole path, which is what `/` and `C:\` have instead of a name, and
/// what a notice should say rather than nothing at all.
pub fn name_of(path: &Path) -> String {
	path.file_name()
		.map(|name| name.to_string_lossy().into_owned())
		.unwrap_or_else(|| path.display().to_string())
}

/// The one thing in this module worth a test: everything else is a `read_dir` with the
/// errors turned into strings, and the walk is the one place a rule lives (PLAN §11b).
#[cfg(test)]
mod tests {
	use super::*;

	#[test]
	fn the_walk_finds_media_at_every_depth_and_nothing_else() {
		// Arrange: an album inside a folder, with a sleeve and a text file to be ignored.
		let root = std::env::temp_dir().join("clecta-tree-test");
		let _ = std::fs::remove_dir_all(&root);
		let album = root.join("album");
		std::fs::create_dir_all(&album).expect("making the test tree");

		for (folder, name) in [
			(&root, "top.mp3"),
			(&root, "notes.txt"),
			(&album, "02-second.flac"),
			(&album, "10-tenth.flac"),
			(&album, "sleeve.jpg"),
		] {
			std::fs::write(folder.join(name), b"x").expect("writing a fixture");
		}

		// Act
		let found = media_tree(&root).expect("a readable root");

		// Assert: media only, at both depths, in the order the pane would list them — and the
		// numbers compared as numbers, since the walk sorts the way the browser does.
		let names: Vec<String> = found.iter().map(|path| name_of(path)).collect();
		assert_eq!(names, ["02-second.flac", "10-tenth.flac", "top.mp3"]);

		// And an unreadable root is an error rather than an empty answer, which would be
		// indistinguishable from a folder with no music in it.
		assert!(media_tree(&root.join("nowhere")).is_err());

		let _ = std::fs::remove_dir_all(&root);
	}
}
