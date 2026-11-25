use anyhow::Result;
use clap::Parser;

/// MigrationX CLI — PostgreSQL migration risk analysis and simulation.
#[derive(Parser, Debug)]
#[command(name = "migrationx", version, about)]
struct Cli {
    /// Path to the migration SQL file to analyze.
    #[arg(short, long)]
    migration: Option<String>,
}

fn main() -> Result<()> {
    let _cli = Cli::parse();

    // Verify crate wiring is correct
    tracing::info!(
        static_analyzer_version = migrationx_static_analyzer::version(),
        simulator_version = migrationx_simulator::version(),
        "MigrationX CLI initialized"
    );

    Ok(())
}
