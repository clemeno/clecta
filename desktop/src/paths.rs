//! Where clecta is allowed to write (PLAN §11).
//!
//! Portability is the requirement, not a preference: copy the app anywhere, run it, and
//! everything it writes lands in `clecta-data/` beside it. The per-user folder is the
//! fallback for an app dropped somewhere read-only, not the normal case.
//!
//! Plain `std`, no `dirs` crate — two environment variables cover both targets.

use std::path::{Path, PathBuf};

/// The one folder clecta writes to. The name is deliberately obvious in a file listing:
/// someone who copies the app somewhere should be able to see what it left behind.
const DATA_DIR: &str = "clecta-data";

/// The folder holding `settings.json`, created if it does not exist.
///
/// Portable first, per-user second. Resolved on each call rather than cached: it is two
/// `stat`s and a probe write, called twice in a run.
pub fn data_dir() -> PathBuf {
	if let Some(portable) = std::env::current_exe()
		.ok()
		.as_deref()
		.and_then(portable_dir)
		.map(|dir| dir.join(DATA_DIR))
		&& is_writable(&portable)
	{
		return portable;
	}

	per_user()
}

/// The directory a portable `clecta-data/` belongs in, given where the executable is.
///
/// Beside the executable — except inside a macOS bundle, where `current_exe()` is
/// `Clecta.app/Contents/MacOS/clecta` and "beside the executable" would mean *inside*
/// the bundle: wiped by any app replacement, and enough to invalidate a code signature.
/// Three levels up is the folder a user sees the app sitting in (PLAN §11).
///
/// `ponytail:` a path-suffix test, not a bundle API. Wrong only for a binary a user has
/// hand-placed in a directory tree that mimics `Something.app/Contents/MacOS`.
fn portable_dir(exe: &Path) -> Option<PathBuf> {
	let dir = exe.parent()?;

	if dir.ends_with("Contents/MacOS")
		&& let Some(bundle) = dir.parent().and_then(Path::parent)
		&& bundle
			.extension()
			.is_some_and(|extension| extension == "app")
	{
		return bundle.parent().map(Path::to_path_buf);
	}

	Some(dir.to_path_buf())
}

/// Can we actually write here? Asked by writing, because a read-only mount, a missing
/// permission and a full disk all look identical until something is written.
fn is_writable(dir: &Path) -> bool {
	if std::fs::create_dir_all(dir).is_err() {
		return false;
	}

	let probe = dir.join(".write-probe");
	let writable = std::fs::write(&probe, b"").is_ok();
	let _ = std::fs::remove_file(&probe);
	writable
}

/// The fallback: the platform's per-user application-data folder. Only reached when the
/// app itself sits somewhere unwritable — `/Applications`, `Program Files`, a read-only
/// image.
fn per_user() -> PathBuf {
	#[cfg(windows)]
	let base = std::env::var_os("LOCALAPPDATA")
		.map(PathBuf::from)
		.unwrap_or_else(std::env::temp_dir);

	// `fsio::home` already falls back to a listable path, so there is nothing left to
	// guard against here.
	#[cfg(not(windows))]
	let base = crate::fsio::home().join("Library/Application Support");

	base.join("clecta")
}

#[cfg(test)]
mod tests {
	use super::*;

	#[test]
	fn an_ordinary_binary_keeps_its_data_beside_itself() {
		// Arrange / Act / Assert: the USB-stick case, which is the one the requirement is
		// about.
		assert_eq!(
			portable_dir(Path::new("/Volumes/stick/clecta")),
			Some(PathBuf::from("/Volumes/stick"))
		);
	}

	#[test]
	fn a_bundled_binary_climbs_out_of_the_app() {
		// Arrange / Act / Assert: beside the `.app`, not inside it — the wrinkle PLAN §11
		// exists to name.
		assert_eq!(
			portable_dir(Path::new("/Volumes/stick/Clecta.app/Contents/MacOS/clecta")),
			Some(PathBuf::from("/Volumes/stick"))
		);
	}

	#[test]
	fn a_lookalike_path_that_is_not_a_bundle_does_not_climb() {
		// Arrange / Act / Assert: the walk-up needs all three of Contents, MacOS and the
		// `.app` extension, so a folder merely named `Contents/MacOS` is left alone.
		assert_eq!(
			portable_dir(Path::new("/src/Contents/MacOS/clecta")),
			Some(PathBuf::from("/src/Contents/MacOS"))
		);
	}
}
