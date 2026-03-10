use clap::Parser;

/// The envelope id argument parser.
#[derive(Debug, Parser)]
pub struct HeaderRawArgs {
    /// Prefill the template with custom headers.
    ///
    /// A raw header should follow the pattern KEY:VAL.
    #[arg(long = "header", short = 'H', required = false)]
    #[arg(name = "header-raw", value_name = "KEY:VAL", value_parser = raw_header_parser)]
    pub raw: Vec<(String, String)>,
}

pub fn raw_header_parser(raw_header: &str) -> Result<(String, String), String> {
    if let Some((key, val)) = raw_header.split_once(':') {
        Ok((key.trim().to_owned(), val.trim().to_owned()))
    } else {
        Err(format!("cannot parse raw header {raw_header:?}"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn simple_header() {
        assert_eq!(
            raw_header_parser("Subject: hello"),
            Ok(("Subject".into(), "hello".into()))
        );
    }

    #[test]
    fn trims_whitespace() {
        assert_eq!(
            raw_header_parser("  Key  :  Val  "),
            Ok(("Key".into(), "Val".into()))
        );
    }

    #[test]
    fn splits_on_first_colon_only() {
        assert_eq!(
            raw_header_parser("X-Custom: a:b:c"),
            Ok(("X-Custom".into(), "a:b:c".into()))
        );
    }

    #[test]
    fn no_colon_is_error() {
        assert!(raw_header_parser("nocolon").is_err());
    }

    #[test]
    fn empty_value() {
        assert_eq!(raw_header_parser("Key:"), Ok(("Key".into(), "".into())));
    }
}
