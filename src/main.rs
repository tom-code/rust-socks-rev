use clap::{Parser, Subcommand};

mod pipe_srv;
mod gw;
mod socks;
mod wire;

#[derive(Parser)]
#[command()]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    Socks {
        #[arg(default_value_t=("0.0.0.0:9000").to_string())]
        pipe_listen_addr: String,

        #[arg(default_value_t=("0.0.0.0:8000").to_string())]
        socks_listen_addr: String
    },
    Gw {
        #[arg(default_value_t=("127.0.0.1:9000").to_string())]
        socks_pipe_addr: String
    },
}

fn main() {
    env_logger::Builder::new()
    .target(env_logger::Target::Stdout)
    .filter_level(log::LevelFilter::Debug)
    .format_timestamp(Some(env_logger::TimestampPrecision::Millis))
    .init();

    let cli = Cli::parse();
    match &cli.command {
        Commands::Socks { pipe_listen_addr, socks_listen_addr } => {
            socks::start_socks_server(&pipe_listen_addr, &socks_listen_addr);
        }
        Commands::Gw { socks_pipe_addr } => {
            gw::gw_start(&socks_pipe_addr);
        }
    }
}
