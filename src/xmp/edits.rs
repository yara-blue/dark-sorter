use std::hash::{DefaultHasher, Hash, Hasher};
use std::num::ParseIntError;

use crate::xmp::edits::helpers::ParserHelper;

use itertools::Itertools;
use miette::{Diagnostic, SourceSpan};
use thiserror::Error;

mod helpers;

pub(crate) fn parse_edits(s: &str) -> Result<Vec<Edit>, miette::Report> {
    fn inner(s: &str) -> Result<Vec<Edit>, ParseFieldsError> {
        let mut s = ParserHelper::new(s);
        let Ok(_) = s.advance_beyond(r"<darktable:history>") else {
            return Ok(Vec::new());
        };
        if s.advance_beyond(r"<rdf:Seq>").is_err() {
            // xmp uses having only the end tag `<rfd:Seq/>` to signal an empty sequence
            return Ok(Vec::new());
        }
        s.accept_one_or_more_whitespace()?;
        std::iter::from_fn(|| s.accept_field().transpose())
            .try_collect()
            .map_err(ParseFieldsError::Field)
    }

    inner(s).map_err(|error| miette::Report::new(error).with_source_code(s.to_string()))
}

#[derive(Diagnostic, Error, Clone, Debug, PartialEq, Eq)]
enum ParseFieldsError {
    #[error("Could not parse one of the edits")]
    Field(
        #[diagnostic_source]
        #[from]
        ParseFieldError,
    ),
    #[error(transparent)]
    #[diagnostic(transparent)]
    WhiteSpace(#[from] helpers::AcceptMinOneWhitespaceError),
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Edit {
    num: usize,
    operation: Operation,
    enabled: bool,
    pub hash: u64,
}

#[derive(Diagnostic, Error, Clone, Debug, PartialEq, Eq)]
enum ParseFieldError {
    #[error("Could not parse number of edit operation")]
    CouldNotParseNum {
        #[label]
        label: SourceSpan,
        #[source]
        error: ParseIntError,
    },
    #[error("Could not parse whether operation is enabled, expected `1` or `0`")]
    ParseEnabled(#[label] SourceSpan),
    #[error(transparent)]
    #[diagnostic(transparent)]
    AcceptNum(#[from] helpers::AcceptError),
    #[error(transparent)]
    #[diagnostic(transparent)]
    WhiteSpace(#[from] helpers::AcceptMinOneWhitespaceError),
    #[error(transparent)]
    #[diagnostic(transparent)]
    CouldNotFindFieldEnd(helpers::AdvanceBeyondError),
}

impl ParserHelper<'_> {
    fn accept_field(&mut self) -> Result<Option<Edit>, ParseFieldError> {
        if self.accept("<rdf:li").is_err() {
            return Ok(None);
        }
        self.accept_one_or_more_whitespace()?;
        let start = self.span_at_current_pos();
        self.accept("darktable:num=\"")?;
        let num = self.accept_until_char('"');
        let num = num
            .parse()
            .map_err(|error| ParseFieldError::CouldNotParseNum {
                label: self.span_till_current_pos(num),
                error,
            })?;

        self.advance_char().expect("peeked for accept until");
        self.accept_one_or_more_whitespace()?;
        self.accept("darktable:operation=\"")?;
        let operation = self.accept_until_char('"');
        let operation = parse_operation(operation);

        self.advance_char().expect("peeked for accept until");
        self.accept_one_or_more_whitespace()?;
        self.accept("darktable:enabled=\"")?;
        let enabled = self.accept_until_char('"');
        let enabled = match enabled {
            "0" => false,
            "1" => true,
            _ => {
                return Err(ParseFieldError::ParseEnabled(
                    self.span_till_current_pos(enabled),
                ));
            }
        };

        // Fields seem to forbid /> and the like so this is probably safe
        let end = self
            .advance_beyond(r"/>")
            .map_err(ParseFieldError::CouldNotFindFieldEnd)?;
        self.accept_one_or_more_whitespace()?;

        Ok(Some(Edit {
            num,
            operation,
            enabled,
            hash: self.hash(start..end),
        }))
    }

    pub(crate) fn hash(&self, range: std::ops::Range<SourceSpan>) -> u64 {
        let edit_text = &self.input[range.start.offset()..range.end.offset()];
        let mut hasher = DefaultHasher::new();
        edit_text.hash(&mut hasher);
        hasher.finish()
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
enum Render {
    Demosaic,
    RawPrepare,
    Gamma,
    ColorOut,
    ColorIn,
    Flip,
    ChannelMixerRGB,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
enum PostProcess {
    Highlights,
    Temperature,
    LenseCorrection,
    Agx,
    Exposure,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
enum Operation {
    PostProcess(PostProcess),
    Render(Render),
    Unknown(String),
}

fn parse_operation(s: &str) -> Operation {
    match s {
        "rawprepare" => Operation::Render(Render::RawPrepare),
        "demosaic" => Operation::Render(Render::Demosaic),
        "colorin" => Operation::Render(Render::ColorIn),
        "colorout" => Operation::Render(Render::ColorOut),
        "gamma" => Operation::Render(Render::Gamma),
        "flip" => Operation::Render(Render::Flip),
        "channelmixerrgb" => Operation::Render(Render::ChannelMixerRGB),

        "temperature" => Operation::PostProcess(PostProcess::Temperature),
        "highlights" => Operation::PostProcess(PostProcess::Highlights),
        "agx" => Operation::PostProcess(PostProcess::Agx),
        "exposure" => Operation::PostProcess(PostProcess::Exposure),
        "lens" => Operation::PostProcess(PostProcess::LenseCorrection),

        other => Operation::Unknown(other.to_string()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn non_empty() -> miette::Result<()> {
        use Render::*;
        use PostProcess::*;

        let xmp = include_str!("../../tests/assets/small_raw.NEF.xmp");
        assert_eq!(
            parse_edits(xmp)?,
            vec![
                Edit {
                    num: 0,
                    operation: Operation::Render(RawPrepare),
                    enabled: true,
                    hash: 11006611376185176428
                },
                Edit {
                    num: 1,
                    operation: Operation::Render(Demosaic),
                    enabled: true,
                    hash: 5898785988624095154
                },
                Edit {
                    num: 2,
                    operation: Operation::Render(ColorIn),
                    enabled: true,
                    hash: 2212034700558132485
                },
                Edit {
                    num: 3,
                    operation: Operation::Render(ColorOut),
                    enabled: true,
                    hash: 6755857187102319893
                },
                Edit {
                    num: 4,
                    operation: Operation::Render(Gamma),
                    enabled: true,
                    hash: 4095472566387997674
                },
                Edit {
                    num: 5,
                    operation: Operation::PostProcess(Temperature),
                    enabled: true,
                    hash: 18301230132671716235
                },
                Edit {
                    num: 6,
                    operation: Operation::PostProcess(Highlights),
                    enabled: true,
                    hash: 6091024285588825961
                },
                Edit {
                    num: 7,
                    operation: Operation::PostProcess(Agx),
                    enabled: true,
                    hash: 5419273352085062930
                },
                Edit {
                    num: 8,
                    operation: Operation::Render(ChannelMixerRGB),
                    enabled: true,
                    hash: 14425096563900339426
                },
                Edit {
                    num: 9,
                    operation: Operation::PostProcess(Exposure),
                    enabled: true,
                    hash: 2615864803797558207
                },
                Edit {
                    num: 10,
                    operation: Operation::Render(Flip),
                    enabled: true,
                    hash: 4563662528553371068
                },
                Edit {
                    num: 11,
                    operation: Operation::PostProcess(LenseCorrection),
                    enabled: true,
                    hash: 4635566713411306703
                }
            ]
        );
        Ok(())
    }
}
