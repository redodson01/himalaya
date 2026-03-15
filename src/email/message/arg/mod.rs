use clap::Parser;

pub mod body;
pub mod header;
pub mod reply;

/// The raw message argument parser.
#[derive(Debug, Parser)]
pub struct MessageRawArg {
    /// The raw message, including headers and body.
    #[arg(trailing_var_arg = true)]
    #[arg(name = "message_raw", value_name = "MESSAGE")]
    pub raw: Vec<String>,
}

impl MessageRawArg {
    pub fn raw(self) -> String {
        self.raw.join(" ").replace('\r', "").replace('\n', "\r\n")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_raw() {
        let arg = MessageRawArg { raw: vec![] };
        assert_eq!(arg.raw(), "");
    }

    #[test]
    fn joins_and_normalizes() {
        let arg = MessageRawArg {
            raw: vec!["Subject: test\n\nbody".into()],
        };
        assert_eq!(arg.raw(), "Subject: test\r\n\r\nbody");
    }
}
