use std::path::PathBuf;
use std::process;

use clap::{Parser, Subcommand};

#[derive(Parser)]
#[clap(name = "hubullu", about = "Hubullu compiler — .hu to .huc")]
struct Cli {
    /// Increase log verbosity (-v info, -vv debug, -vvv trace)
    #[clap(short = 'v', long = "verbose", action = clap::ArgAction::Count, global = true)]
    verbose: u8,

    #[clap(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Compile a .hu file to a .huc file
    Compile {
        /// Entry point .hu file
        input: PathBuf,

        /// Output .huc file
        #[clap(short, long, default_value = "dictionary.huc")]
        output: PathBuf,
    },
    /// Render a .hut token list
    Render {
        /// Input .hut file (single-file mode)
        input: Option<PathBuf>,

        /// Directory of .hut files to render as a static HTML site
        #[clap(long, short = 'd')]
        dir: Option<PathBuf>,

        /// Output directory for HTML site (required with --dir)
        #[clap(long, short = 'o')]
        outdir: Option<PathBuf>,

        /// Pre-compiled .huc file to use for resolution (skips .hu compilation)
        #[clap(long)]
        huc: Option<PathBuf>,

        /// Site title (used for index.html page title and navigation label)
        #[clap(long)]
        title: Option<String>,
    },
    /// Lint .hu files for warnings and style issues
    Lint {
        /// Entry point .hu file
        input: PathBuf,

        /// Automatically fix issues where possible
        #[clap(long)]
        fix: bool,
    },
    /// Start the Language Server Protocol server
    #[cfg(feature = "lsp")]
    Lsp,
    /// Manage Claude Code skills bundled with hubullu
    Skill {
        #[clap(subcommand)]
        action: SkillAction,
    },
}

#[derive(Subcommand)]
enum SkillAction {
    /// List bundled skills and their install status
    List,
    /// Show the content of a bundled skill
    Show {
        /// Skill name
        name: String,
    },
    /// Install skills into a project or globally
    Install {
        /// Skill name (omit to install all)
        name: Option<String>,

        /// Install into the current project (.claude/skills/)
        #[clap(long, group = "scope")]
        project: bool,

        /// Install globally (~/.claude/skills/)
        #[clap(long, group = "scope")]
        global: bool,
    },
    /// Uninstall skills from a project or globally
    Uninstall {
        /// Skill name (omit to uninstall all)
        name: Option<String>,

        /// Uninstall from the current project
        #[clap(long, group = "scope")]
        project: bool,

        /// Uninstall globally
        #[clap(long, group = "scope")]
        global: bool,
    },
}

fn main() {
    let cli = Cli::parse();

    if cli.verbose > 0 {
        let level = match cli.verbose {
            1 => log::LevelFilter::Info,
            2 => log::LevelFilter::Debug,
            _ => log::LevelFilter::Trace,
        };
        env_logger::Builder::new()
            .filter_module("hubullu", level)
            .format_timestamp_millis()
            .init();
    }

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
        Command::Lint { input, fix } => {
            let result = hubullu::lint::run_lint(&input);

            if result.compile_errors.has_errors() {
                eprintln!("{}", result.compile_errors.render_all(&result.source_map));
                process::exit(1);
            }

            if !result.has_lints() {
                eprintln!("No lint issues found.");
                return;
            }

            eprint!("{}", result.render_all());

            if fix {
                match hubullu::lint::apply_fixes(&result.lints, &result.source_map) {
                    Ok(n) => {
                        eprintln!("Fixed {} issue(s).", n);
                    }
                    Err(e) => {
                        eprintln!("error applying fixes: {}", e);
                        process::exit(1);
                    }
                }
            }

            let unfixed = result.lints.iter().filter(|l| {
                if fix { l.fix.is_none() } else { true }
            }).count();
            if unfixed > 0 {
                process::exit(1);
            }
        }
        Command::Render { input, dir, outdir, huc, title } => {
            if let Some(dir) = dir {
                // Site mode: render all .hut files under dir to HTML.
                let outdir = match outdir {
                    Some(o) => o,
                    None => {
                        eprintln!("error: --outdir is required with --dir");
                        process::exit(1);
                    }
                };
                match hubullu::render_html::render_site(
                    &dir,
                    &outdir,
                    huc.as_deref(),
                    title.as_deref(),
                ) {
                    Ok(()) => {}
                    Err(msg) => {
                        eprintln!("{}", msg);
                        process::exit(1);
                    }
                }
            } else if let Some(input) = input {
                // Single-file mode: render to stdout (existing behavior).
                let source = match std::fs::read_to_string(&input) {
                    Ok(s) => s,
                    Err(e) => {
                        eprintln!("cannot read '{}': {}", input.display(), e);
                        process::exit(1);
                    }
                };

                let (hut_file, hut_source_map) = match hubullu::render::parse_hut(&source, &input.to_string_lossy()) {
                    Ok(h) => h,
                    Err(msg) => {
                        eprintln!("{}", msg);
                        process::exit(1);
                    }
                };

                let hut_dir = input
                    .canonicalize()
                    .ok()
                    .and_then(|p| p.parent().map(|d| d.to_path_buf()))
                    .unwrap_or_else(|| std::env::current_dir().unwrap_or_default());

                let ctx = if let Some(huc_path) = huc {
                    match hubullu::render::ResolveContext::from_huc(
                        &hut_file.references,
                        &hut_dir,
                        &huc_path,
                    ) {
                        Ok(c) => c,
                        Err(msg) => {
                            eprintln!("{}", msg);
                            process::exit(1);
                        }
                    }
                } else {
                    match hubullu::render::ResolveContext::from_references(
                        &hut_file.references,
                        &hut_dir,
                    ) {
                        Ok(c) => c,
                        Err(msg) => {
                            eprintln!("{}", msg);
                            process::exit(1);
                        }
                    }
                };

                let parts = match hubullu::render::resolve(&hut_file.tokens, &ctx, &hut_source_map) {
                    Ok(p) => p,
                    Err(msg) => {
                        eprintln!("{}", msg);
                        process::exit(1);
                    }
                };

                let (separator, no_sep_before) = hubullu::render::read_render_config(&ctx);
                let output = hubullu::render::smart_join(&parts, &separator, &no_sep_before);
                println!("{}", output);
            } else {
                eprintln!("error: provide an input .hut file or use --dir");
                process::exit(1);
            }
        }
        #[cfg(feature = "lsp")]
        Command::Lsp => {
            hubullu::lsp::run_server();
        }
        Command::Skill { action } => {
            let result = match action {
                SkillAction::List => hubullu::skill::list(),
                SkillAction::Show { name } => hubullu::skill::show(&name),
                SkillAction::Install { name, project, global } => {
                    if !project && !global {
                        eprintln!("error: specify --project or --global");
                        process::exit(1);
                    }
                    hubullu::skill::install(name.as_deref(), project, global)
                }
                SkillAction::Uninstall { name, project, global } => {
                    if !project && !global {
                        eprintln!("error: specify --project or --global");
                        process::exit(1);
                    }
                    hubullu::skill::uninstall(name.as_deref(), project, global)
                }
            };

            if let Err(msg) = result {
                eprintln!("error: {}", msg);
                process::exit(1);
            }
        }
    }
}
