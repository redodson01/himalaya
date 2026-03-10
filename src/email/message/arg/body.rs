use clap::Parser;
use std::ops::Deref;

/// The raw message body argument parser.
#[derive(Debug, Parser)]
pub struct MessageRawBodyArg {
    /// Prefill the template with a custom body.
    #[arg(trailing_var_arg = true)]
    #[arg(name = "body_raw", value_name = "BODY")]
    pub raw: Vec<String>,
}

impl MessageRawBodyArg {
    pub fn raw(self) -> String {
        self.raw.join(" ").replace('\r', "").replace('\n', "\r\n")
    }
}

impl Deref for MessageRawBodyArg {
    type Target = Vec<String>;

    fn deref(&self) -> &Self::Target {
        &self.raw
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_body() {
        let arg = MessageRawBodyArg { raw: vec![] };
        assert_eq!(arg.raw(), "");
    }

    #[test]
    fn joins_with_spaces() {
        let arg = MessageRawBodyArg {
            raw: vec!["hello".into(), "world".into()],
        };
        assert_eq!(arg.raw(), "hello world");
    }

    #[test]
    fn normalizes_newlines() {
        let arg = MessageRawBodyArg {
            raw: vec!["line1\nline2".into()],
        };
        assert_eq!(arg.raw(), "line1\r\nline2");
    }

    #[test]
    fn strips_bare_cr() {
        let arg = MessageRawBodyArg {
            raw: vec!["hello\rworld".into()],
        };
        // \r is stripped first, then \n→\r\n doesn't apply (no \n)
        assert_eq!(arg.raw(), "helloworld");
    }
}
