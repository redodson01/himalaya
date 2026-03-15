use clap::Parser;
use email::flag::{Flag, Flags};
use tracing::debug;

/// The ids and/or flags arguments parser.
#[derive(Debug, Parser)]
pub struct IdsAndFlagsArgs {
    /// The list of ids and/or flags.
    ///
    /// Every argument that can be parsed as an integer is considered
    /// an id, otherwise it is considered as a flag.
    #[arg(value_name = "ID-OR-FLAG", required = true)]
    pub ids_and_flags: Vec<IdOrFlag>,
}

#[derive(Clone, Debug, Eq, PartialEq, Ord, PartialOrd)]
pub enum IdOrFlag {
    Id(usize),
    Flag(Flag),
}

impl From<&str> for IdOrFlag {
    fn from(value: &str) -> Self {
        value.parse::<usize>().map(Self::Id).unwrap_or_else(|err| {
            let flag = Flag::from(value);
            debug!("cannot parse {value} as usize, parsing it as flag {flag}");
            debug!("{err:?}");
            Self::Flag(flag)
        })
    }
}

pub fn into_tuple(ids_and_flags: &[IdOrFlag]) -> (Vec<usize>, Flags) {
    ids_and_flags.iter().fold(
        (Vec::default(), Flags::default()),
        |(mut ids, mut flags), arg| {
            match arg {
                IdOrFlag::Id(id) => {
                    ids.push(*id);
                }
                IdOrFlag::Flag(flag) => {
                    flags.insert(flag.to_owned());
                }
            };
            (ids, flags)
        },
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn id_or_flag_from_numeric_string() {
        assert_eq!(IdOrFlag::from("42"), IdOrFlag::Id(42));
    }

    #[test]
    fn id_or_flag_from_zero() {
        assert_eq!(IdOrFlag::from("0"), IdOrFlag::Id(0));
    }

    #[test]
    fn id_or_flag_from_negative_is_flag() {
        assert!(matches!(IdOrFlag::from("-1"), IdOrFlag::Flag(_)));
    }

    #[test]
    fn id_or_flag_from_word_is_flag() {
        assert!(matches!(IdOrFlag::from("seen"), IdOrFlag::Flag(_)));
    }

    #[test]
    fn into_tuple_empty() {
        let (ids, flags) = into_tuple(&[]);
        assert!(ids.is_empty());
        assert!(flags.is_empty());
    }

    #[test]
    fn into_tuple_mixed() {
        let input = vec![
            IdOrFlag::from("1"),
            IdOrFlag::from("seen"),
            IdOrFlag::from("2"),
            IdOrFlag::from("flagged"),
        ];
        let (ids, flags) = into_tuple(&input);
        assert_eq!(ids, vec![1, 2]);
        assert_eq!(flags.len(), 2);
    }

    #[test]
    fn into_tuple_only_ids() {
        let input = vec![IdOrFlag::from("10"), IdOrFlag::from("20")];
        let (ids, flags) = into_tuple(&input);
        assert_eq!(ids, vec![10, 20]);
        assert!(flags.is_empty());
    }

    #[test]
    fn into_tuple_only_flags() {
        let input = vec![IdOrFlag::from("seen"), IdOrFlag::from("flagged")];
        let (ids, flags) = into_tuple(&input);
        assert!(ids.is_empty());
        assert_eq!(flags.len(), 2);
    }
}
