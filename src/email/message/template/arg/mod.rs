pub mod body;

use clap::Parser;

/// The raw template argument parser.
#[derive(Debug, Parser)]
pub struct TemplateRawArg {
    /// The raw template, including headers and MML body.
    #[arg(trailing_var_arg = true)]
    #[arg(name = "template_raw", value_name = "TEMPLATE")]
    pub raw: Vec<String>,
}

impl TemplateRawArg {
    pub fn raw(self) -> String {
        self.raw.join(" ").replace('\r', "")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_template() {
        let arg = TemplateRawArg { raw: vec![] };
        assert_eq!(arg.raw(), "");
    }

    #[test]
    fn strips_cr_only() {
        let arg = TemplateRawArg {
            raw: vec!["line1\r\nline2".into()],
        };
        // Only strips \r, does NOT add \r\n
        assert_eq!(arg.raw(), "line1\nline2");
    }

    #[test]
    fn preserves_lf() {
        let arg = TemplateRawArg {
            raw: vec!["line1\nline2".into()],
        };
        assert_eq!(arg.raw(), "line1\nline2");
    }
}
