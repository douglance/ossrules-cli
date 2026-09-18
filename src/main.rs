//! The `ossrules` binary.
//!
//! The command graph lives in the library so tests drive the same definitions
//! this process serves, through `serve_to`, rather than a parallel copy.
//!
//! Deliberately not `#[tokio::main]`: keeping the runtime explicit leaves room
//! for a surface that builds its own, which panics if one is already running.

fn main() -> Result<(), Box<dyn std::error::Error>> {
    tokio::runtime::Runtime::new()?.block_on(ossrules_cli::build_cli().serve())
}
