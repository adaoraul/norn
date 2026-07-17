//! norn: a terminal IRCv3 client built on the norn engine.
//!
//! Step 1 wires up configuration only: it parses the command line, resolves the
//! bring-up config, and prints a summary. The real transport and event loop
//! arrive in the following steps.

mod config;

use clap::Parser;

use config::Cli;

fn main() {
    let settings = Cli::parse().into_settings();

    let scheme = if settings.conn.tls { "ircs" } else { "irc" };
    println!(
        "norn: would connect to {scheme}://{}:{} as {}",
        settings.conn.host, settings.conn.port, settings.bringup.nick
    );
    println!(
        "  user={} realname={:?}",
        settings.bringup.user, settings.bringup.realname
    );
    match &settings.sasl_account {
        Some(account) if !settings.bringup.sasl.is_empty() => {
            println!("  SASL: PLAIN as {account}");
        }
        _ => println!("  SASL: disabled"),
    }
    println!(
        "  requesting {} capability group(s)",
        settings.bringup.cap_groups.len()
    );
}
