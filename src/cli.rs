use clap::{Parser, Subcommand, ValueEnum};

#[derive(Parser)]
#[command(author, version, about)]
pub struct Cli {
    #[arg(short, long, global = true, default_value_t = 1)]
    pub interval: u64,

    #[command(subcommand)]
    pub command: Option<Command>,
}

#[derive(Subcommand)]
pub enum Command {
    /// Monitor local Intel CPU/GPU/NPU devices. This is the default.
    Local {
        #[arg(short, long, default_value_t = 1)]
        interval: u64,
        #[arg(long)]
        once: bool,
    },
    /// Emit a scriptable one-shot sample.
    Snapshot {
        #[arg(long, value_enum, default_value_t = OutputFormat::Json)]
        format: OutputFormat,
        #[arg(long, default_value_t = 1)]
        samples: u32,
        #[arg(short, long, default_value_t = 1)]
        interval: u64,
        /// Sub-second sampling interval in milliseconds (overrides --interval).
        #[arg(long)]
        interval_ms: Option<u32>,
        /// Emit one row per active process per sample (csv/prometheus).
        #[arg(long)]
        processes: bool,
    },
    /// Expose a small Prometheus-compatible /metrics endpoint.
    Api {
        #[arg(short, long, default_value_t = 9090)]
        port: u16,
        #[arg(short, long, default_value_t = 1)]
        interval: u64,
    },
    /// Check local prerequisites and counter visibility.
    Doctor,
}

#[derive(Copy, Clone, Debug, ValueEnum)]
pub enum OutputFormat {
    Json,
    Csv,
    Prometheus,
    Nvidia,
}
