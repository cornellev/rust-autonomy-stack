//! Cargo-style terminal output: a bold, right-aligned status word, then the
//! message. Colour is dropped automatically when stderr is not a terminal or
//! `NO_COLOR` is set.

use std::fmt::Display;

use anstyle::{AnsiColor, Style};

const STATUS: Style = AnsiColor::Green.on_default().bold();
const ERROR: Style = AnsiColor::Red.on_default().bold();

/// `   Checking car sim`: the verb right-aligned in 12-columns, like Cargo.
pub fn status(verb: &str, message: impl Display) {
    anstream::eprintln!("{STATUS}{verb:>12}{STATUS:#} {message}");
}

/// `error: ...`, fojr a problem that does not stop the command.
pub fn error(message: impl Display) {
    anstream::eprintln!("{ERROR}error{ERROR:#}: {message}");
}
