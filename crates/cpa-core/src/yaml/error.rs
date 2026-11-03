//! The error of every [`crate::yaml`] operation. Callers only display it.

use std::fmt::{self, Debug, Display};

/// A YAML parse, conversion or emit failure.
pub struct Error(Box<ErrorImpl>);

pub(crate) enum ErrorImpl {
    Message(String),
    ScalarInMerge,
    TaggedInMerge,
    ScalarInMergeElement,
    SequenceInMergeElement,
    EmptyTag,
    FailedToParseNumber,
}

pub(crate) fn new(inner: ErrorImpl) -> Error {
    Error(Box::new(inner))
}

impl Error {
    pub(crate) fn msg(msg: impl Display) -> Self {
        new(ErrorImpl::Message(msg.to_string()))
    }
}

impl std::error::Error for Error {}

impl Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        // The texts serde_yaml_ng gave the same failures.
        match self.0.as_ref() {
            ErrorImpl::Message(msg) => f.write_str(msg),
            ErrorImpl::ScalarInMerge => {
                f.write_str("expected a mapping or list of mappings for merging, but found scalar")
            }
            ErrorImpl::TaggedInMerge => f.write_str("unexpected tagged value in merge"),
            ErrorImpl::ScalarInMergeElement => f.write_str("expected a mapping for merging, but found scalar"),
            ErrorImpl::SequenceInMergeElement => f.write_str("expected a mapping for merging, but found sequence"),
            ErrorImpl::EmptyTag => f.write_str("empty YAML tag is not allowed"),
            ErrorImpl::FailedToParseNumber => f.write_str("failed to parse YAML number"),
        }
    }
}

impl Debug for Error {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(f, "Error({:?})", self.to_string())
    }
}

impl serde::de::Error for Error {
    fn custom<T: Display>(msg: T) -> Self {
        Error::msg(msg)
    }
}

impl serde::ser::Error for Error {
    fn custom<T: Display>(msg: T) -> Self {
        Error::msg(msg)
    }
}
