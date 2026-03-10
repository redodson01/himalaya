pub mod account;
pub mod cli;
pub mod completion;
pub mod config;
pub mod email;
pub mod folder;
pub mod manual;

use std::path::PathBuf;

use shellexpand_utils::{canonicalize, expand};

#[doc(inline)]
pub use crate::email::{envelope, flag, message};

/// Parse the given [`str`] as [`PathBuf`].
///
/// The path is first shell expanded, then canonicalized (if
/// applicable).
fn dir_parser(path: &str) -> Result<PathBuf, String> {
    expand::try_path(path)
        .map(canonicalize::path)
        .map_err(|err| err.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dir_parser_current_dir() {
        let result = dir_parser(".");
        assert!(result.is_ok());
        assert!(result.unwrap().is_absolute());
    }

    #[test]
    fn dir_parser_tilde() {
        let result = dir_parser("~");
        assert!(result.is_ok());
        let path = result.unwrap();
        assert!(path.is_absolute());
        assert!(!path.to_string_lossy().contains('~'));
    }
}
