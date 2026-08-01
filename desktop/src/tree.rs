//! The folder tree's model (PLAN §9): which folders exist, which are open, and which
//! still need reading.
//!
//! Pure, like `browser`. The rule that shapes everything here is that `expand` does not
//! read the filesystem — it *asks* for a read and returns the paths to read, so an
//! unreadable or enormous folder cannot stall the GUI thread.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

/// One folder in the tree.
#[derive(Debug, Clone)]
pub struct Node {
	pub name: String,
	pub expanded: bool,
	/// `None` = never listed. `Some(vec![])` = listed and genuinely empty.
	///
	/// Collapsing those two into one empty vector is what makes a permission-denied
	/// folder re-request itself on every redraw (PLAN §9).
	pub children: Option<Vec<PathBuf>>,
}

/// A row to draw: a folder, and how deep it sits.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Row {
	pub path: PathBuf,
	pub name: String,
	pub depth: usize,
	pub expanded: bool,
	/// Whether to draw a disclosure arrow at all. Unknown until the folder is listed, so
	/// an unlisted folder gets one on the assumption it might have children — the
	/// alternative is listing every folder in the tree up front.
	pub expandable: bool,
}

/// The folder tree.
#[derive(Debug, Default)]
pub struct Tree {
	nodes: HashMap<PathBuf, Node>,
	roots: Vec<PathBuf>,
}

impl Tree {
	/// Build a tree over the platform's roots — one on macOS, one per drive letter on
	/// Windows (PLAN §9). Nothing is listed yet.
	pub fn new(roots: Vec<PathBuf>) -> Self {
		let mut tree = Self {
			nodes: HashMap::new(),
			roots: roots.clone(),
		};
		for root in roots {
			tree.insert(root);
		}
		tree
	}

	/// Add a node for a path if there is not one already, leaving any cached children
	/// alone.
	fn insert(&mut self, path: PathBuf) {
		self.nodes.entry(path.clone()).or_insert_with(|| Node {
			name: display_name(&path),
			expanded: false,
			children: None,
		});
	}

	/// Open a folder. Returns the paths that need a listing before the tree under it is
	/// accurate — which is always the folder itself: cached children draw immediately so
	/// re-opening is instant, and the fresh listing replaces them when it lands, because
	/// a folder the user deliberately opens should show what is there *now* (PLAN §9).
	pub fn expand(&mut self, path: &Path) -> Vec<PathBuf> {
		self.insert(path.to_path_buf());
		if let Some(node) = self.nodes.get_mut(path) {
			node.expanded = true;
		}
		vec![path.to_path_buf()]
	}

	/// Close a folder. The subtree goes with it visually, but every cached listing stays,
	/// so re-opening draws instantly (PLAN §9).
	pub fn collapse(&mut self, path: &Path) {
		if let Some(node) = self.nodes.get_mut(path) {
			node.expanded = false;
		}
	}

	pub fn toggle(&mut self, path: &Path) -> Vec<PathBuf> {
		if self.is_expanded(path) {
			self.collapse(path);
			Vec::new()
		} else {
			self.expand(path)
		}
	}

	pub fn is_expanded(&self, path: &Path) -> bool {
		self.nodes.get(path).is_some_and(|node| node.expanded)
	}

	/// Accept a listing for one folder.
	pub fn set_children(&mut self, path: &Path, children: Vec<PathBuf>) {
		for child in &children {
			self.insert(child.clone());
		}
		self.insert(path.to_path_buf());
		if let Some(node) = self.nodes.get_mut(path) {
			node.children = Some(children);
		}
	}

	/// Open every folder on the way down to `path`, so a folder chosen elsewhere (the
	/// **Open folder…** dialog) appears in the tree where it belongs.
	///
	/// Returns exactly the paths that were never listed and therefore need reading —
	/// unlike `expand`, this is a navigation side effect rather than a deliberate open,
	/// so there is no reason to re-read what is already cached.
	pub fn reveal(&mut self, path: &Path) -> Vec<PathBuf> {
		let mut chain: Vec<PathBuf> = path.ancestors().map(Path::to_path_buf).collect();
		chain.reverse();

		let mut unlisted = Vec::new();
		for folder in chain {
			self.insert(folder.clone());
			let needs_listing = self
				.nodes
				.get(&folder)
				.is_some_and(|node| node.children.is_none());

			if let Some(node) = self.nodes.get_mut(&folder) {
				// The destination itself is revealed, not opened: opening it would show
				// its subfolders, which is a decision for the user to make.
				node.expanded = folder != path;
			}
			if needs_listing {
				unlisted.push(folder);
			}
		}
		unlisted
	}

	/// The visible rows, depth-first, in the order they are drawn.
	pub fn rows(&self) -> Vec<Row> {
		let mut rows = Vec::new();
		for root in &self.roots {
			self.push_rows(root, 0, &mut rows);
		}
		rows
	}

	fn push_rows(&self, path: &Path, depth: usize, rows: &mut Vec<Row>) {
		let Some(node) = self.nodes.get(path) else {
			return;
		};

		rows.push(Row {
			path: path.to_path_buf(),
			name: node.name.clone(),
			depth,
			expanded: node.expanded,
			// Unlisted folders get an arrow on spec; listed empty ones lose it.
			expandable: node.children.as_ref().is_none_or(|kids| !kids.is_empty()),
		});

		if node.expanded
			&& let Some(children) = &node.children
		{
			for child in children {
				self.push_rows(child, depth + 1, rows);
			}
		}
	}
}

/// What to call a folder in the tree. The file name, except for a root, which has none —
/// `/` on macOS, `C:\` on Windows.
fn display_name(path: &Path) -> String {
	path.file_name()
		.map(|name| name.to_string_lossy().into_owned())
		.unwrap_or_else(|| path.display().to_string())
}

#[cfg(test)]
mod tests {
	use super::*;

	/// A tree rooted at `/`, with `/music` and `/music/set` already listed under it.
	fn listed_tree() -> Tree {
		let mut tree = Tree::new(vec![PathBuf::from("/")]);
		tree.set_children(Path::new("/"), vec![PathBuf::from("/music")]);
		tree.set_children(Path::new("/music"), vec![PathBuf::from("/music/set")]);
		tree
	}

	fn paths(rows: &[Row]) -> Vec<String> {
		rows.iter()
			.map(|row| row.path.display().to_string())
			.collect()
	}

	#[test]
	fn a_new_tree_shows_its_roots_and_nothing_else() {
		// Arrange / Act
		let tree = Tree::new(vec![PathBuf::from("/")]);

		// Assert: nothing has been read, so nothing below the root can be known.
		assert_eq!(paths(&tree.rows()), ["/"]);
	}

	#[test]
	fn expanding_asks_for_a_fresh_listing_even_when_one_is_cached() {
		// Arrange
		let mut tree = listed_tree();

		// Act
		let needed = tree.expand(Path::new("/music"));

		// Assert: a folder the user deliberately opens shows what is there now, not what
		// was there last time (PLAN §9).
		assert_eq!(needed, [PathBuf::from("/music")]);
	}

	#[test]
	fn collapsing_takes_the_subtree_with_it_and_keeps_the_listings() {
		// Arrange: root and /music both open, so /music/set is on screen.
		let mut tree = listed_tree();
		let _ = tree.expand(Path::new("/"));
		let _ = tree.expand(Path::new("/music"));
		assert_eq!(paths(&tree.rows()), ["/", "/music", "/music/set"]);

		// Act
		tree.collapse(Path::new("/"));

		// Assert: the whole subtree goes...
		assert_eq!(paths(&tree.rows()), ["/"]);

		// ...but re-opening draws it again with no new listing.
		let _ = tree.expand(Path::new("/"));
		assert_eq!(paths(&tree.rows()), ["/", "/music", "/music/set"]);
	}

	#[test]
	fn never_listed_and_listed_but_empty_stay_different() {
		// Arrange: two folders, one read and found empty, one never read.
		let mut tree = Tree::new(vec![PathBuf::from("/")]);
		tree.set_children(
			Path::new("/"),
			vec![PathBuf::from("/empty"), PathBuf::from("/unread")],
		);
		tree.set_children(Path::new("/empty"), vec![]);
		let _ = tree.expand(Path::new("/"));

		// Act
		let rows = tree.rows();
		let arrow = |path: &str| {
			rows.iter()
				.find(|row| row.path == Path::new(path))
				.expect("row is on screen")
				.expandable
		};

		// Assert: the empty one loses its arrow, the unread one keeps it — which is the
		// whole reason `children` is an `Option` (PLAN §9).
		assert!(!arrow("/empty"), "listed and empty");
		assert!(arrow("/unread"), "never listed");
	}

	#[test]
	fn revealing_a_folder_opens_its_ancestors_but_not_the_folder_itself() {
		// Arrange
		let mut tree = listed_tree();

		// Act
		let _ = tree.reveal(Path::new("/music/set"));

		// Assert: the destination is visible, and its own contents are not forced open.
		assert_eq!(paths(&tree.rows()), ["/", "/music", "/music/set"]);
		assert!(!tree.is_expanded(Path::new("/music/set")));
	}

	#[test]
	fn revealing_asks_only_for_the_ancestors_that_were_never_listed() {
		// Arrange: `/` and `/music` are cached, `/music/set` is not.
		let mut tree = listed_tree();

		// Act
		let needed = tree.reveal(Path::new("/music/set"));

		// Assert: exactly the unlisted one — re-reading the cached ancestors would be
		// filesystem work nobody asked for.
		assert_eq!(needed, [PathBuf::from("/music/set")]);
	}
}
