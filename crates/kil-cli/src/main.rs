use anyhow::Result;
use clap::{Parser, Subcommand, ValueEnum};
use kil_core::diagnostic::render_text;
use kil_core::{
    BuildOptions, BuildOutcome, InspectOptions, InspectOutcome, RouteOptions, RouteOutcome,
};
use std::path::PathBuf;

#[derive(Debug, Parser)]
#[command(
    name = "kil",
    version,
    about = "Compiler for the KiCad Intent Language"
)]
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
    /// Accept the current resolved symbol and footprint contents.
    Lock { file: PathBuf },
    /// Compile and publish a KiCad project.
    Build {
        file: PathBuf,
        #[arg(long)]
        out: Option<PathBuf>,
    },
    /// Route a staged board with KiCadRoutingTools and publish a safe route cache.
    Route {
        file: PathBuf,
        /// Override the bundled KiCadRoutingTools root or py_router/route.py.
        #[arg(long)]
        krt: Option<PathBuf>,
        /// Override the bundled Python environment used by KiCadRoutingTools.
        #[arg(long)]
        python: Option<PathBuf>,
        /// Override build.routing.nets; repeat for multiple patterns.
        #[arg(long = "net")]
        nets: Vec<String>,
        /// Route all nets owned by one imported block.
        #[arg(long, conflicts_with = "nets")]
        block: Option<String>,
    },
    /// Print a compact, read-only JSON view of a project, block, net, component or PCB region.
    Inspect {
        file: PathBuf,
        #[arg(long, conflicts_with_all = ["net", "block", "region"])]
        component: Option<String>,
        #[arg(long, conflicts_with_all = ["component", "block", "region"])]
        net: Option<String>,
        #[arg(long, conflicts_with_all = ["component", "net", "region"])]
        block: Option<String>,
        /// PCB rectangle in local millimetres: x1 y1 x2 y2.
        #[arg(long, num_args = 4, conflicts_with_all = ["component", "net", "block"])]
        region: Option<Vec<f64>>,
    },
    /// Print the JSON Schema for format_version 2.
    Schema {
        /// Print the schema for imported module files instead of root projects.
        #[arg(long)]
        module: bool,
    },
    /// Query the KiCad libraries visible to KIL.
    Library {
        #[command(subcommand)]
        command: LibraryCommands,
    },
}

#[derive(Debug, Subcommand)]
enum LibraryCommands {
    /// Show a symbol's inherited properties, pin numbers, names and electrical types.
    Show { symbol: String },
}

fn main() -> Result<()> {
    let cli = Cli::parse();
    match cli.command {
        Commands::Schema { module } => {
            let schema = if module {
                kil_core::module_schema()
            } else {
                kil_core::schema()
            };
            println!("{}", serde_json::to_string_pretty(&schema)?);
            Ok(())
        }
        Commands::Library {
            command: LibraryCommands::Show { symbol },
        } => {
            let project_dir = std::env::current_dir()?;
            let resolver = kil_core::library::LibraryResolver::discover(
                &project_dir,
                cli.kicad_cli.as_deref(),
            );
            let info = resolver.show_symbol(&symbol).map_err(anyhow::Error::msg)?;
            println!("{}", serde_json::to_string_pretty(&info)?);
            Ok(())
        }
        Commands::Lock { file } => finish_lock(&file, cli.kicad_cli.as_deref(), cli.diagnostics),
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
            block,
        } => finish_route(
            kil_core::route(&RouteOptions {
                input: file,
                kicad_cli: cli.kicad_cli,
                krt,
                python,
                nets,
                block,
            }),
            cli.diagnostics,
        ),
        Commands::Inspect {
            file,
            component,
            net,
            block,
            region,
        } => {
            let region = region.map(|values| [values[0], values[1], values[2], values[3]]);
            finish_inspect(
                kil_core::inspect(&InspectOptions {
                    input: file,
                    component,
                    net,
                    block,
                    region,
                }),
                cli.diagnostics,
            )
        }
    }
}

fn finish_lock(
    file: &std::path::Path,
    kicad: Option<&std::path::Path>,
    format: OutputFormat,
) -> Result<()> {
    let loaded = kil_core::load_project(file);
    let mut diagnostics = loaded.diagnostics;
    let mut lock_file = None;
    if !diagnostics
        .iter()
        .any(|d| d.severity == kil_core::Severity::Error)
        && let Some(project) = loaded.project
    {
        let resolver = kil_core::library::LibraryResolver::discover(
            file.parent().unwrap_or(std::path::Path::new(".")),
            kicad,
        );
        let (libraries, library_diagnostics) = resolver.resolve_all(&project, file, &loaded.source);
        diagnostics.extend(library_diagnostics);
        if !diagnostics
            .iter()
            .any(|d| d.severity == kil_core::Severity::Error)
        {
            match kil_core::lock::write(file, &libraries) {
                Ok(path) => lock_file = Some(path),
                Err(e) => diagnostics.push(kil_core::Diagnostic::error("LOCK004", e, file)),
            }
        }
    }
    let success = lock_file.is_some();
    match format {
        OutputFormat::Json => println!(
            "{}",
            serde_json::to_string_pretty(
                &serde_json::json!({"exit":if success {"success"}else{"invalid"},"diagnostics":diagnostics,"lock_file":lock_file})
            )?
        ),
        OutputFormat::Text => {
            eprint!("{}", render_text(&diagnostics, Some(&loaded.source)));
            if let Some(path) = lock_file {
                println!("locked {}", path.display());
            }
        }
    }
    std::process::exit(if success { 0 } else { 1 });
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

fn finish_inspect(outcome: InspectOutcome, format: OutputFormat) -> Result<()> {
    match format {
        OutputFormat::Json => println!("{}", serde_json::to_string_pretty(&outcome)?),
        OutputFormat::Text => {
            if !outcome.diagnostics.is_empty() {
                eprint!(
                    "{}",
                    render_text(&outcome.diagnostics, Some(&outcome.source))
                );
            }
            if let Some(data) = &outcome.data {
                println!("{}", serde_json::to_string_pretty(data)?);
            }
        }
    }
    std::process::exit(outcome.exit.code());
}
