use clap::Parser;
use color_eyre::Result;
use himalaya::{
    cli::{Cli, HimalayaCommand},
    config::TomlConfig,
    envelope::command::{list::EnvelopeListCommand, EnvelopeSubcommand},
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
        // SAFETY: Called before the tokio runtime starts; no other
        // threads exist yet. The `unused_unsafe` allow keeps this
        // compiling on edition 2021 while being forward-compatible
        // with edition 2024 where `set_var` becomes unsafe.
        #[allow(unused_unsafe)]
        unsafe {
            std::env::set_var("RUST_LOG", "warn,imap_codec=error");
        }
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
            cmd.execute(&mut printer, cli.config_paths.as_ref(), cli.all)
                .await
        }
        None => {
            let cmd =
                HimalayaCommand::Envelope(EnvelopeSubcommand::List(EnvelopeListCommand::default()));
            cmd.execute(&mut printer, cli.config_paths.as_ref(), cli.all)
                .await
        }
    };

    tracing.with_debug_and_trace_notes(res)
}
