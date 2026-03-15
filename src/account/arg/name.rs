use clap::Parser;

/// The account name argument parser.
#[derive(Debug, Parser)]
pub struct AccountNameArg {
    /// The name of the account.
    ///
    /// An account name corresponds to an entry in the table at the
    /// root level of your TOML configuration file.
    #[arg(name = "account_name", value_name = "ACCOUNT")]
    pub name: String,
}

/// The optional account name argument parser.
#[derive(Debug, Parser)]
pub struct OptionalAccountNameArg {
    /// The name of the account.
    ///
    /// An account name corresponds to an entry in the table at the
    /// root level of your TOML configuration file.
    ///
    /// If omitted, the account marked as default will be used.
    #[arg(name = "account_name", value_name = "ACCOUNT")]
    pub name: Option<String>,
}

/// Container for the account name, populated from the global
/// `--account` / `-a` flag on the top-level `Cli` struct before
/// command execution. This struct is kept for backward compatibility
/// with command code that reads `self.account.name`.
#[derive(Clone, Debug, Default, Parser)]
pub struct AccountNameFlag {
    #[arg(skip)]
    pub name: Option<String>,
}
