use std::fmt::Display;

fn no_color() -> bool {
    std::env::var_os("NO_COLOR").is_some()
}

pub fn ok(msg: impl Display) {
    if no_color() {
        println!("[ ok ] {msg}");
    } else {
        println!("\x1b[32m[ ok ]\x1b[0m {msg}");
    }
}

pub fn fail(msg: impl Display) {
    if no_color() {
        println!("[fail] {msg}");
    } else {
        println!("\x1b[31m[fail]\x1b[0m {msg}");
    }
}

pub fn warn(msg: impl Display) {
    if no_color() {
        println!("[warn] {msg}");
    } else {
        println!("\x1b[33m[warn]\x1b[0m {msg}");
    }
}

pub fn header(text: impl Display) {
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

pub fn next_step(cmd: impl Display) {
    if no_color() {
        println!("next: {cmd}");
    } else {
        println!("\x1b[1mnext:\x1b[0m {cmd}");
    }
}
