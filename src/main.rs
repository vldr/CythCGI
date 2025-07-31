extern crate fastcgi;

use std::{io::Write, net::TcpListener};

fn main() {
    let listener = TcpListener::bind("127.0.0.1:1237").unwrap();

    fastcgi::run_tcp(
        |mut req| {
            let mut target_string = String::new();
            use std::fmt::Write;

            writeln!(target_string, "Content-Type: text/plain\n\n").unwrap();

            for param in req.params() {
                writeln!(target_string, "{:?}", param).unwrap();
            }

            write!(&mut req.stdout(), "{}", target_string).unwrap_or(());
        },
        &listener,
    );
}
