use anyhow::Result;
use clap::{Parser, Subcommand, ValueEnum};
use kil_core::diagnostic::render_text;
use kil_core::{BuildOptions, BuildOutcome, RouteOptions, RouteOutcome};
use std::path::PathBuf;

#[derive(Debug, Parser)]
#[command(name = "kil", version, about = "Safe, deterministic KiCad IL compiler")]
struct Cli {
    #[arg(long, value_enum, default_value_t = OutputFormat::Text, global = true)]
    diagnostics: OutputFormat,
    #[arg(long, global = true)]
    kicad_cli: Option<PathBuf>,
    #[command(subcommand)]
    command: Commands,
}

#[derive(Debug, Clone, Copy, ValueEnum)]
enum OutputFormat {
    Text,
    Json,
}

#[derive(Debug, Subcommand)]
enum Commands {
    /// Validate IL, resolve libraries, generate in a temporary directory and run KiCad checks.
    Check { file: PathBuf },
    /// Compile and publish a KiCad project.
    Build {
        file: PathBuf,
        #[arg(long)]
        out: Option<PathBuf>,
    },
    /// Route a staged board with KiCadRoutingTools and publish a safe route cache.
    Route {
        file: PathBuf,
        /// KiCadRoutingTools repository root or py_router/route.py.
        #[arg(long)]
        krt: Option<PathBuf>,
        /// Python 3.9+ executable used to run KiCadRoutingTools.
        #[arg(long)]
        python: Option<PathBuf>,
        /// Override pcb.routing.nets; repeat for multiple patterns.
        #[arg(long = "net")]
        nets: Vec<String>,
    },
    /// Print the JSON Schema for format_version 1.
    Schema,
}

fn main() -> Result<()> {
    let cli = Cli::parse();
    match cli.command {
        Commands::Schema => {
            println!("{}", serde_json::to_string_pretty(&kil_core::schema())?);
            Ok(())
        }
        Commands::Check { file } => finish(
            kil_core::check(&BuildOptions {
                input: file,
                output: None,
                kicad_cli: cli.kicad_cli,
            }),
            cli.diagnostics,
        ),
        Commands::Build { file, out } => finish(
            kil_core::build(&BuildOptions {
                input: file,
                output: out,
                kicad_cli: cli.kicad_cli,
            }),
            cli.diagnostics,
        ),
        Commands::Route {
            file,
            krt,
            python,
            nets,
        } => finish_route(
            kil_core::route(&RouteOptions {
                input: file,
                kicad_cli: cli.kicad_cli,
                krt,
                python,
                nets,
            }),
            cli.diagnostics,
        ),
    }
}

fn finish(outcome: BuildOutcome, format: OutputFormat) -> Result<()> {
    match format {
        OutputFormat::Json => println!("{}", serde_json::to_string_pretty(&outcome)?),
        OutputFormat::Text => {
            if outcome.diagnostics.is_empty() {
                if let Some(path) = &outcome.output_dir {
                    println!("built {}", path.display());
                } else {
                    println!("check passed");
                }
            } else {
                eprint!(
                    "{}",
                    render_text(&outcome.diagnostics, Some(&outcome.source))
                );
                if let Some(path) = &outcome.output_dir {
                    println!("built {}", path.display());
                }
            }
        }
    }
    std::process::exit(outcome.exit.code());
}

fn finish_route(outcome: RouteOutcome, format: OutputFormat) -> Result<()> {
    match format {
        OutputFormat::Json => println!("{}", serde_json::to_string_pretty(&outcome)?),
        OutputFormat::Text => {
            if !outcome.diagnostics.is_empty() {
                eprint!(
                    "{}",
                    render_text(&outcome.diagnostics, Some(&outcome.source))
                );
            }
            if let Some(path) = &outcome.cache_file {
                println!("routed {}", path.display());
            }
        }
    }
    std::process::exit(outcome.exit.code());
}
