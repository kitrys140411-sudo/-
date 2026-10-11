//! lumen-term: the terminal emulator of Lumen. The library holds everything
//! that does not need a display (emulation core, PTY, configuration), so it
//! can be tested and benchmarked headless; the Wayland/GL front end is in the
//! binary.

pub mod config;
pub mod input;
pub mod layout;
pub mod pty;
pub mod select;
pub mod term;
