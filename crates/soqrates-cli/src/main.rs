//! The `soqrates` binary (ADR-0001 D1).
//!
//! Usage:
//!   soqrates hook pre-commit   # git
//!   soqrates hook pre-push     # jj
//!   soqrates ci --repo <owner/repo> --pr <number>
//!   soqrates mcp               # MCP server over stdio

use std::process::ExitCode;

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match args.first().map(String::as_str) {
        Some("hook" | "ci" | "mcp") => {
            eprintln!("soqrates: `{}` is not implemented yet", args.join(" "));
            ExitCode::from(2)
        }
        _ => {
            eprintln!("usage: soqrates <hook pre-commit|hook pre-push|ci|mcp>");
            ExitCode::from(64)
        }
    }
}
