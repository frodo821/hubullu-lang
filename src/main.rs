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

        /// F4: Additional `.hut` code evaluated as if appended to the input
        /// file. Repeatable: `-e A -e B` is equivalent to `-e "A; B"`.
        /// `@file:<path>` reads the code from a file (curl-style sugar).
        #[clap(
            short = 'e',
            long = "eval",
            value_name = "CODE",
            action = clap::ArgAction::Append,
        )]
        eval: Vec<String>,
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
        Command::Render { input, dir, outdir, huc, title, eval } => {
            if let Some(dir) = dir {
                if !eval.is_empty() {
                    eprintln!("error: -e/--eval is not supported with --dir");
                    process::exit(1);
                }
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

                let (hut_file, hut_source_map) = match hubullu::render::parse_hut_with_eval(
                    &source,
                    &input.to_string_lossy(),
                    &eval,
                ) {
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

                // Phase 4: the lazy-compose render path needs the
                // `HutPhonContext` (entry/inflection AST + phonrule resolver)
                // *during* `resolve()`. Pre-scan the token tree to decide
                // whether to pre-build the context — non-slot-spec renders
                // keep the old fast path that skips phase1/phase2 entirely.
                let needs_lazy_render =
                    hubullu::render::tokens_need_phon_ctx(&hut_file.tokens);
                let early_phon_ctx = if needs_lazy_render {
                    match hubullu::render::HutPhonContext::build(&hut_file, &hut_dir) {
                        Ok(c) => Some(c),
                        Err(msg) => {
                            eprintln!("{}", msg);
                            process::exit(1);
                        }
                    }
                } else {
                    None
                };

                let parts = match hubullu::render::resolve_with_phon_ctx(
                    &hut_file.tokens,
                    &ctx,
                    early_phon_ctx.as_ref(),
                    &hut_source_map,
                ) {
                    Ok(p) => p,
                    Err(msg) => {
                        eprintln!("{}", msg);
                        process::exit(1);
                    }
                };

                // F1b: apply file-level `@apply` phonrule chain. F1c: also
                // dispatch on inline `phon_call` / `@apply { ... }` markers
                // even when the file-level chain is empty.
                let needs_phonrules = !hut_file.apply_chain.is_empty()
                    || parts.iter().any(|p| {
                        matches!(
                            p,
                            hubullu::render::ResolvedPart::PhonCallStart(_)
                                | hubullu::render::ResolvedPart::ApplyBlockStart(_)
                        )
                    });
                let parts = if !needs_phonrules {
                    parts
                } else {
                    // Reuse the early context if we already built one; the
                    // existing build helper is idempotent / cheap to re-run
                    // but we'd prefer not to.
                    let owned_ctx;
                    let phon_ctx_ref = if let Some(c) = early_phon_ctx.as_ref() {
                        c
                    } else {
                        match hubullu::render::HutPhonContext::build(&hut_file, &hut_dir) {
                            Ok(c) => {
                                owned_ctx = c;
                                &owned_ctx
                            }
                            Err(msg) => {
                                eprintln!("{}", msg);
                                process::exit(1);
                            }
                        }
                    };
                    match hubullu::render::apply_phonrule_chain(
                        parts,
                        &hut_file.apply_chain,
                        &phon_ctx_ref.resolver(),
                        &hut_source_map,
                    ) {
                        Ok(p) => p,
                        Err(msg) => {
                            eprintln!("{}", msg);
                            process::exit(1);
                        }
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
