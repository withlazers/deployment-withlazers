mod result;
mod subcommands;

use crate::result::Result;
use clap::{Parser, Subcommand};

#[derive(Parser, Debug)]
struct Args {
    #[arg(short, long)]
    verbose: bool,
    #[command(subcommand)]
    subcommand: Action,
}

#[derive(Subcommand, Debug, Clone)]
enum Action {
    Pipeline(subcommands::pipeline::Args),
}

fn main() -> Result<()> {
    let args = Args::parse();

    match args.subcommand {
        Action::Pipeline(pipeline) => subcommands::pipeline::run(pipeline),
    }
}
