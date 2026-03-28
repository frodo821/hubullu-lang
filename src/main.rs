use std::path::PathBuf;
use std::process;

use clap::Parser;

#[derive(Parser)]
#[clap(name = "hubullu", about = "LexDSL compiler")]
struct Cli {
    /// Entry point .hu file
    input: PathBuf,

    /// Output SQLite file
    #[clap(short, long, default_value = "dictionary.sqlite")]
    output: PathBuf,
}

fn main() {
    let cli = Cli::parse();

    match hubullu::compile(&cli.input, &cli.output) {
        Ok(()) => {
            eprintln!("Compiled to {}", cli.output.display());
        }
        Err(msg) => {
            eprintln!("{}", msg);
            process::exit(1);
        }
    }
}
