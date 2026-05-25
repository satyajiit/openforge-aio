//! Terminal output helpers. `NO_COLOR` env var disables ANSI codes.

use std::fmt::Display;

fn no_color() -> bool {
    std::env::var_os("NO_COLOR").is_some()
}

pub fn ok(label: &str) {
    if no_color() {
        println!("[ ok ] {label}");
    } else {
        println!("\x1b[32m[ ok ]\x1b[0m {label}");
    }
}

pub fn fail(label: &str, reason: impl Display) {
    if no_color() {
        println!("[fail] {label}: {reason}");
    } else {
        println!("\x1b[31m[fail]\x1b[0m {label}: {reason}");
    }
}

pub fn warn(label: &str, reason: impl Display) {
    if no_color() {
        println!("[warn] {label}: {reason}");
    } else {
        println!("\x1b[33m[warn]\x1b[0m {label}: {reason}");
    }
}

pub fn header(text: &str) {
    if no_color() {
        println!("== {text} ==");
    } else {
        println!("\x1b[1m== {text} ==\x1b[0m");
    }
}

pub fn dim(text: impl Display) {
    if no_color() {
        println!("{text}");
    } else {
        println!("\x1b[2m{text}\x1b[0m");
    }
}

pub fn bullet(text: impl Display) {
    println!("  • {text}");
}

pub fn next_step(cmd: impl Display) {
    if no_color() {
        println!("\nnext: {cmd}");
    } else {
        println!("\n\x1b[1mnext:\x1b[0m {cmd}");
    }
}
