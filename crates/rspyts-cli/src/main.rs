//! Process entry point for the `rspyts` executable.
//!
//! The library owns parsing and behavior. The binary owns only terminal error
//! presentation and the nonzero failure exit status.

fn main() {
    if let Err(error) = rspyts_cli::run() {
        eprintln!("error: {error:#}");
        std::process::exit(1);
    }
}
