extern crate rsdns;

use std::{fs, process::exit};

use clap::{Args, Parser, Subcommand};
use tokio_util::sync::CancellationToken;

#[derive(Debug, Parser)]
#[command(about = rsdns::build_info::PKG_DESCRIPTION, long_about = None)]
struct Cli {
    #[command(flatten)]
    args: GlobalArgs,

    #[command(subcommand)]
    command: Commands,
}

#[derive(Debug, Args)]
struct GlobalArgs {
    #[clap(help = "config path", short, long, default_value = "config.yaml")]
    config: String,
}

#[derive(Debug, Subcommand)]
enum Commands {
    #[clap(about = "show version")]
    Version,
    #[clap(about = "run the server")]
    Run,
    #[clap(about = "show supported features")]
    ShowSupport,
}

fn main() {
    let cli = Cli::parse();
    match cli.command {
        Commands::Version => {
            println!(
                "rsdns v{}-{}",
                rsdns::build_info::PKG_VERSION,
                rsdns::build_info::SHORT_COMMIT
            );
        }
        Commands::Run => {
            let rt = tokio::runtime::Builder::new_multi_thread()
                .enable_all()
                .build()
                .unwrap();
            if rt.block_on(run(cli.args.config)).is_err() {
                exit(1);
            }
        }
        Commands::ShowSupport => {
            show_support();
        }
    }
}

async fn prepare_run(config_file: String) -> Result<rsdns::Manager, ()> {
    let config_content = match fs::read_to_string(config_file) {
        Ok(v) => v,
        Err(e) => {
            eprintln!("failed to read config file: {}", e);
            return Err(());
        }
    };
    let options: rsdns::Options = match serde_yaml::from_str(&config_content) {
        Ok(v) => v,
        Err(e) => {
            eprintln!("failed to parse config file: {}", e);
            return Err(());
        }
    };
    let manager = match rsdns::Manager::prepare(options).await {
        Ok(v) => v,
        Err(e) => {
            eprintln!("failed to create service: {}", e);
            return Err(());
        }
    };
    Ok(manager)
}

async fn run(config_file: String) -> Result<(), ()> {
    let manager = prepare_run(config_file).await?;
    let token = CancellationToken::new();
    let token_handle = token.clone();
    ctrlc::set_handler(move || {
        token_handle.cancel();
    })
    .ok();
    match manager.run(token).await {
        Ok(_) => Ok(()),
        Err(e) => {
            eprintln!("{}", e);
            Err(())
        }
    }
}

fn show_support() {
    let mut s = String::new();
    s.push_str(format!("support socks5 dialer: {}", rsdns::support_socks5_dialer()).as_str());
    s.push('\n');
    s.push_str(
        format!(
            "support upstream type: {}",
            rsdns::supported_upstream_types().join(", ")
        )
        .as_str(),
    );
    s.push('\n');
    s.push_str(
        format!(
            "support listener type: {}",
            rsdns::supported_listener_types().join(", ")
        )
        .as_str(),
    );
    s.push('\n');
    s.push_str(
        format!(
            "support matcher plugin type: {}",
            rsdns::support_matcher_plugins().join(", ")
        )
        .as_str(),
    );
    s.push('\n');
    s.push_str(
        format!(
            "support executor plugin type: {}",
            rsdns::support_executor_plugins().join(", ")
        )
        .as_str(),
    );
    s.push('\n');
    s.push_str(format!("support api: {}", rsdns::support_api()).as_str());
    println!("{}", s);
}
