//! Heavily inspired by the work of Jana (@jdonszelmann)
//! https://github.com/jdonszelmann/parse-helper/blob/main/src/byte.rs
//!
//! This is basically her work but then:
//!     - without the byte support
//!     - without methods I did not need
//!     - with advance_char and advance_beyond added
//!     - with some names changed
//!     - with errors and spans bolted on though Miette

use core::fmt;

use miette::{Diagnostic, GraphicalReportHandler, LabeledSpan, SourceSpan, miette};
use thiserror::Error;

#[derive(Diagnostic, Error, Debug, PartialEq, Eq)]
#[error("Could not advance as we are at the end of input")]
#[diagnostic()]
pub struct AdvanceError(#[label] SourceSpan);

impl From<PeekError> for AdvanceError {
    fn from(peek: PeekError) -> Self {
        Self(peek.0)
    }
}

#[derive(Diagnostic, Error, Clone, Debug, PartialEq, Eq)]
#[error("Can not get the upcoming char since we are at the end of the input")]
#[diagnostic()]
pub struct PeekError(#[label] SourceSpan);

#[derive(Diagnostic, Error, Clone, Debug, PartialEq, Eq)]
#[diagnostic()]
pub enum AcceptMinOneWhitespaceError {
    #[error("No whitespace")]
    NoUpcomingChar(#[from] PeekError),
    #[error("There is not a single whitespace character (require one or more)")]
    NoWhitespace(#[label] SourceSpan),
}

#[derive(Diagnostic, Error, Clone, Debug, PartialEq, Eq)]
pub enum AcceptError {
    #[error("Not enough characters left in input to match self")]
    #[diagnostic()]
    NotEnoughCharsLeft(#[label] SourceSpan),
    #[error("Next text did not match `{required}`")]
    #[diagnostic()]
    DoesNotMatch {
        #[label]
        label: SourceSpan,
        required: String,
    },
}

#[derive(Diagnostic, Error, Clone, Debug, PartialEq, Eq)]
#[diagnostic()]
#[error("Required text ({required}) was not in the leftover input")]
pub struct AdvanceBeyondError {
    #[label]
    label: SourceSpan,
    required: String,
}

pub struct ParserHelper<'a> {
    pub input: &'a str,
    byte_pos: usize,
}

impl fmt::Debug for ParserHelper<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let fake_error_report = miette!(
            labels = vec![LabeledSpan::at(
                self.byte_pos..self.byte_pos,
                "Parser position"
            ),],
            ""
        )
        .with_source_code(self.input.to_string());
        let mut code_snippet = String::new();
        GraphicalReportHandler::new()
            .with_context_lines(4)
            .without_cause_chain()
            .without_syntax_highlighting()
            .render_report(&mut code_snippet, fake_error_report.as_ref())?;

        let start_of_snippet = code_snippet
            .find('\n')
            .expect("there is always one line end after the 'error'");
        let preview = &code_snippet[start_of_snippet + '\n'.len_utf8()..];
        f.write_str("Parser(\n")?;
        f.write_str(preview)?;
        f.write_str(")")
    }
}

impl<'a> ParserHelper<'a> {
    pub fn new(input: &'a str) -> Self {
        Self { input, byte_pos: 0 }
    }

    pub fn span_till_current_pos(&self, s: &str) -> SourceSpan {
        ((self.byte_pos - s.len())..self.byte_pos).into()
    }

    pub fn span_at_current_pos(&self) -> SourceSpan {
        (self.byte_pos..self.byte_pos).into()
    }

    fn bytes_left(&self) -> usize {
        self.input.len() - self.byte_pos
    }

    fn leftover(&self) -> &'a str {
        &self.input[self.byte_pos..]
    }

    fn upcoming_char(&self) -> Result<char, PeekError> {
        if self.leftover().is_empty() {
            return Err(PeekError((self.byte_pos..self.byte_pos).into()));
        } else {
            Ok(self.leftover().chars().next().unwrap())
        }
    }

    pub fn advance_char(&mut self) -> Result<char, AdvanceError> {
        let c = self.upcoming_char()?;
        self.byte_pos += c.len_utf8();
        Ok(c)
    }

    pub fn accept(&mut self, required: &str) -> Result<&'a str, AcceptError> {
        assert!(!required.is_empty(), "an empty accept makes no sense");
        if required.len() > self.bytes_left() {
            return Err(AcceptError::NotEnoughCharsLeft(
                (self.byte_pos..self.input.len()).into(),
            ));
        }

        let range_in_input = self.byte_pos..self.byte_pos + required.len();
        if &self.input[range_in_input.clone()] == required {
            self.byte_pos = range_in_input.end;
            Ok(&self.input[range_in_input.clone()])
        } else {
            Err(AcceptError::DoesNotMatch {
                label: range_in_input.into(),
                required: required.to_string(),
            })
        }
    }
    pub fn accept_until_char(&mut self, stop_before: char) -> &'a str {
        self.accept_until_char_with(|c| c == stop_before)
    }
    pub fn accept_until_char_with(&mut self, stop_before: impl Fn(char) -> bool) -> &'a str {
        let start = self.byte_pos;
        while let Ok(char) = self.upcoming_char()
            && !stop_before(char)
        {
            self.byte_pos += char.len_utf8();
        }

        self.input.get(start..self.byte_pos).unwrap()
    }

    pub fn accept_one_or_more_whitespace(
        &mut self,
    ) -> Result<&'a str, AcceptMinOneWhitespaceError> {
        if !self.upcoming_char()?.is_whitespace() {
            return Err(AcceptMinOneWhitespaceError::NoWhitespace(
                (self.byte_pos..self.upcoming_char()?.len_utf8()).into(),
            ));
        } else {
            Ok(self.accept_until_char_with(|x| !x.is_whitespace()))
        }
    }

    /// Returns span at the start of required
    pub(crate) fn advance_beyond(
        &mut self,
        required: &str,
    ) -> Result<SourceSpan, AdvanceBeyondError> {
        let Some(start) = self.leftover().find(required) else {
            return Err(AdvanceBeyondError {
                required: required.to_string(),
                label: self.span_at_current_pos(),
            });
        };

        self.byte_pos += start + required.len();
        Ok((self.byte_pos - required.len()..self.byte_pos).into())
    }
}
