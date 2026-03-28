use std::path::PathBuf;
use std::process;

use clap::{Parser, Subcommand};

#[derive(Parser)]
#[clap(name = "hubullu", about = "LexDSL compiler")]
struct Cli {
    #[clap(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Compile a .hu file to SQLite
    Compile {
        /// Entry point .hu file
        input: PathBuf,

        /// Output SQLite file
        #[clap(short, long, default_value = "dictionary.sqlite")]
        output: PathBuf,
    },
    /// Render a .hut token list using a compiled database
    Render {
        /// Input .hut file
        input: PathBuf,

        /// Compiled SQLite database
        #[clap(long)]
        db: PathBuf,
    },
}

fn main() {
    let cli = Cli::parse();

    match cli.command {
        Command::Compile { input, output } => {
            match hubullu::compile(&input, &output) {
                Ok(()) => {
                    eprintln!("Compiled to {}", output.display());
                }
                Err(msg) => {
                    eprintln!("{}", msg);
                    process::exit(1);
                }
            }
        }
        Command::Render { input, db } => {
            let source = match std::fs::read_to_string(&input) {
                Ok(s) => s,
                Err(e) => {
                    eprintln!("cannot read '{}': {}", input.display(), e);
                    process::exit(1);
                }
            };

            let tokens = match hubullu::render::parse_hut(&source, &input.to_string_lossy()) {
                Ok(t) => t,
                Err(msg) => {
                    eprintln!("{}", msg);
                    process::exit(1);
                }
            };

            let conn = match rusqlite::Connection::open_with_flags(
                &db,
                rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
            ) {
                Ok(c) => c,
                Err(e) => {
                    eprintln!("cannot open database '{}': {}", db.display(), e);
                    process::exit(1);
                }
            };

            let parts = match hubullu::render::resolve(&tokens, &conn) {
                Ok(p) => p,
                Err(msg) => {
                    eprintln!("{}", msg);
                    process::exit(1);
                }
            };

            let (separator, no_sep_before) = hubullu::render::read_render_config(&conn);
            let output = hubullu::render::smart_join(&parts, &separator, &no_sep_before);
            println!("{}", output);
        }
    }
}
