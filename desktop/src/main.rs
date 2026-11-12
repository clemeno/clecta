//! clecta — a two-player audio mixer. See `PLAN.md` for the design and the decision log.
//!
//! This file is the entry point and nothing else: the application lives in `app`, and
//! every rule worth reading is in the module that owns it.

// No console window behind the app on Windows; the attribute is inert on macOS (PLAN §11).
#![windows_subsystem = "windows"]

mod app;
mod audio;
mod browser;
mod cache;
mod deck;
mod focus; // which list ⌘A and Escape are talking to
mod fsio;
mod known; // what this run has learned about files, and the door answers arrive through
mod mixer;
mod opus; // the Opus decoder symphonia 0.5 lacks, for WebM
mod paths;
#[cfg(test)]
mod plan; // the facts PLAN.md states that the build can check
mod queue; // one queue: its rows, its selection, its handover rules
mod queues; // the set of three: what is only true of them together
mod select;
mod settings;
mod stretch; // a player's rate, and the file-timeline playhead the player cannot keep itself
mod tree;
mod ui;
mod waveform;

fn main() -> iced::Result {
	app::run()
}
