//! The facts `PLAN.md` states that the build itself can check.
//!
//! The plan is prose written for people, and most of it can only be kept true by reading it.
//! The version table in §3 is the exception: it repeats what `Cargo.toml` pins and
//! `Cargo.lock` resolves, and it went stale once already. A dependency bump now fails here
//! until all three agree.

/// Every direct dependency is pinned at the version the lockfile resolves, and PLAN §3 says so.
///
/// The manifest rule is "written to the exact patch level in use" (PLAN §3), so the pin and
/// the lock must match; the table must name the same version in the dependency's own row.
#[test]
fn the_plan_and_the_lockfile_agree_with_every_pin() {
	// A checkout with `core.autocrlf` turns all three to CRLF, which the `\n`s below would miss.
	let manifest = include_str!("../Cargo.toml").replace("\r\n", "\n");
	let lock = include_str!("../Cargo.lock").replace("\r\n", "\n");
	let plan = include_str!("../PLAN.md").replace("\r\n", "\n");

	let table = plan
		.split("## 3. Tech stack")
		.nth(1)
		.and_then(|rest| rest.split("\n## ").next())
		.expect("PLAN.md has a §3");
	let dependencies = manifest
		.split("\n[dependencies]\n")
		.nth(1)
		.and_then(|rest| rest.split("\n[").next())
		.expect("Cargo.toml has a [dependencies] table");

	for line in dependencies
		.lines()
		.filter(|line| !line.starts_with('#') && line.contains(" = "))
	{
		let name = line.split(" = ").next().unwrap_or_default();
		// The first quoted string: `name = "1.2.3"` or `name = { version = "1.2.3", … }`.
		let pin = line.split('"').nth(1).expect("a quoted version");

		let locked = format!("name = \"{name}\"\nversion = \"{pin}\"\n");
		assert!(
			lock.contains(&locked),
			"Cargo.lock does not resolve {name} to its pin {pin}"
		);

		let named = table.lines().any(|row| {
			row.starts_with('|') && row.contains(&format!("`{name}`")) && row.contains(pin)
		});
		assert!(named, "PLAN §3 does not give {name} as {pin}");
	}
}
