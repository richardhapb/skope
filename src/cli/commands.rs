use clap::{Parser, Subcommand};

/// A fast and flexible profiler for tracking any application and comparing versions
#[derive(Parser, Debug)]
#[command(name="skope", version, about, long_about = None, author="richardhapb")]
pub struct Cli {
    #[command(subcommand)]
    pub command: Commands,
}

#[derive(Subcommand, Debug, PartialEq)]
pub enum Commands {
    /// The executor of the application
    Runner {
        /// The path of the running script (e.g. run_app.sh)
        #[arg(long)]
        script: String,
        /// The tag of the capture
        #[arg(short, long)]
        tag: String,
    },
    /// Start a server listening for the /start and /stop signals and write each difference to a report
    Server,
    /// Start a server to store various application names and generate aggregate data.
    Agg,
    /// Compare two tagged captures
    Diff {
        /// The base capture for comparation
        #[arg(long)]
        base: String,

        /// The HEAD capture for comparation
        #[arg(long)]
        head: String,
    },
}
