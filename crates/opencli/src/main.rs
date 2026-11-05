use clap::Parser;

/// Turn imported OpenAPI specifications into CLI commands.
#[derive(Debug, Parser)]
#[command(name = "opencli", version, about)]
struct Cli {}

fn main() {
    Cli::parse();
}
