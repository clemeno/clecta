//! clecta — a two-player audio mixer. See `PLAN.md` for the design and the decision log.
//!
//! This file is the entry point and nothing else: the application lives in `app`, and
//! every rule worth reading is in the module that owns it.

// No console window behind the app on Windows; the attribute is inert on macOS (PLAN §11).
#![windows_subsystem = "windows"]

mod app;
mod audio;
mod browser;
mod deck;
mod fsio;
mod mixer;
mod tree;
mod ui;

fn main() -> iced::Result {
	app::run()
}
