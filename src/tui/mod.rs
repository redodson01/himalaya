use std::path::PathBuf;

use color_eyre::Result;

pub async fn run(_config_paths: &[PathBuf], _all: bool, _account: Option<String>) -> Result<()> {
    color_eyre::eyre::bail!("TUI is not yet implemented")
}
