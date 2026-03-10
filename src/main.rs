use clap::Parser;
use color_eyre::Result;
use himalaya::{
    cli::Cli, config::TomlConfig, envelope::command::list::EnvelopeListCommand,
    message::command::mailto::MessageMailtoCommand,
};
use pimalaya_tui::terminal::{
    cli::{printer::StdoutPrinter, tracing},
    config::TomlConfig as _,
};

#[tokio::main]
async fn main() -> Result<()> {
    // Set a default log filter that silences expected warnings from
    // imap-codec quirk features (e.g. servers omitting required text
    // fields). Only applies when the user hasn't set RUST_LOG or
    // passed --quiet/--debug/--trace, which override this via
    // pimalaya-tui's tracing::install().
    if std::env::var("RUST_LOG").is_err()
        && !std::env::args().any(|a| a == "--quiet" || a == "--debug" || a == "--trace")
    {
        std::env::set_var("RUST_LOG", "warn,imap_codec=error");
    }

    let tracing = tracing::install()?;

    #[cfg(feature = "keyring")]
    secret::keyring::set_global_service_name("himalaya-cli");

    // if the first argument starts by "mailto:", execute straight the
    // mailto message command
    let mailto = std::env::args()
        .nth(1)
        .filter(|arg| arg.starts_with("mailto:"));

    if let Some(ref url) = mailto {
        let mut printer = StdoutPrinter::default();
        let config = TomlConfig::from_default_paths().await?;

        return MessageMailtoCommand::new(url)?
            .execute(&mut printer, &config)
            .await;
    }

    let cli = Cli::parse();
    let mut printer = StdoutPrinter::new(cli.output);
    let res = match cli.command {
        Some(cmd) => {
            if cli.tui {
                color_eyre::eyre::bail!("--tui cannot be used with subcommands");
            }
            cmd.execute(&mut printer, cli.config_paths.as_ref()).await
        }
        None => {
            if cli.tui {
                himalaya::tui::run(cli.config_paths.as_ref()).await
            } else {
                let config = TomlConfig::from_paths_or_default(cli.config_paths.as_ref()).await?;
                EnvelopeListCommand::default()
                    .execute(&mut printer, &config)
                    .await
            }
        }
    };

    tracing.with_debug_and_trace_notes(res)
}
