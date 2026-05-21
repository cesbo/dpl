use std::io;

use kdl::KdlNode;
use miette::SourceSpan;
use thiserror::Error;

#[derive(Debug, Error, PartialEq, Eq)]
pub enum TemplateError {
    #[error("unterminated reference at position {pos}: missing '}}'")]
    UnterminatedRef { pos: usize },

    #[error("bare '$' at position {pos}")]
    BareDollar { pos: usize },

    #[error("malformed reference at position {pos}: expected '${{ns:name}}'")]
    MalformedRef { pos: usize },

    #[error("unknown namespace '{ns}' at position {pos}")]
    UnknownNamespace { pos: usize, ns: String },

    #[error("invalid name '{name}' for namespace '{ns}' at position {pos}")]
    InvalidName {
        pos: usize,
        ns: String,
        name: String,
    },
}

#[derive(Debug, Error, PartialEq, Eq)]
pub enum FieldError {
    #[error("expected exactly one positional value")]
    EntryCount { span: SourceSpan },

    #[error("named entries are not allowed")]
    NamedEntry { span: SourceSpan },

    #[error("child blocks are not allowed")]
    HasChildren { span: SourceSpan },

    #[error("expected {expected}")]
    InvalidType {
        expected: &'static str,
        span: SourceSpan,
    },

    #[error("expected integer from {min} to {max}")]
    OutOfRange {
        min: i128,
        max: i128,
        span: SourceSpan,
    },

    #[error("invalid template: {source}")]
    InvalidTemplate {
        span: SourceSpan,
        #[source]
        source: TemplateError,
    },

    #[error("expected {expected}")]
    InvalidValue {
        expected: &'static str,
        span: SourceSpan,
    },
}

#[derive(Debug, Error, PartialEq, Eq)]
pub enum NodeError {
    #[error("unexpected argument")]
    UnexpectedArg { span: SourceSpan },

    #[error("missing argument '{name}'")]
    MissingArg {
        name: &'static str,
        span: SourceSpan,
    },

    #[error("unknown field '{name}'")]
    UnknownField { name: String, span: SourceSpan },

    #[error("duplicate field '{name}'")]
    DuplicateField { name: String, span: SourceSpan },

    #[error("missing required field '{name}'")]
    MissingField { name: String, span: SourceSpan },

    #[error("invalid field '{name}'")]
    InvalidField {
        name: String,
        span: SourceSpan,
        #[source]
        source: FieldError,
    },

    #[error("unknown value '{value}' for field '{field}'")]
    UnknownVariant {
        field: String,
        value: String,
        span: SourceSpan,
    },
}

impl NodeError {
    pub fn invalid_field(node: &KdlNode, source: FieldError) -> Self {
        NodeError::InvalidField {
            name: node.name().value().to_owned(),
            span: node.span(),
            source,
        }
    }
}

#[derive(Debug, Error)]
pub enum ConfigError {
    #[error("read config")]
    Read(#[source] io::Error),

    #[error("write config")]
    Write(#[source] io::Error),

    #[error("parse config")]
    Parse(#[source] Box<kdl::KdlError>),

    #[error("invalid config")]
    Semantic(#[source] Box<NodeError>),
}
