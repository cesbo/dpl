use std::io;

use kdl::KdlNode;
use miette::{
    Diagnostic,
    NamedSource,
    SourceSpan,
};
use thiserror::Error;

#[derive(Debug, Error, Diagnostic, PartialEq, Eq)]
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

#[derive(Debug, Error, Diagnostic, PartialEq, Eq)]
pub enum FieldError {
    #[error("expected exactly one positional value")]
    EntryCount {
        #[label("here")]
        span: SourceSpan,
    },

    #[error("named entries are not allowed")]
    NamedEntry {
        #[label("here")]
        span: SourceSpan,
    },

    #[error("child blocks are not allowed")]
    HasChildren {
        #[label("here")]
        span: SourceSpan,
    },

    #[error("expected {expected}")]
    InvalidType {
        expected: &'static str,
        #[label("not a {expected}")]
        span: SourceSpan,
    },

    #[error("expected integer from {min} to {max}")]
    OutOfRange {
        min: i128,
        max: i128,
        #[label("out of range")]
        span: SourceSpan,
    },

    #[error("invalid template: {source}")]
    InvalidTemplate {
        #[label("here")]
        span: SourceSpan,
        #[source]
        source: TemplateError,
    },

    #[error("expected {expected}")]
    InvalidValue {
        expected: &'static str,
        #[label("not a {expected}")]
        span: SourceSpan,
    },
}

#[derive(Debug, Error, Diagnostic, PartialEq, Eq)]
pub enum NodeError {
    #[error("unexpected argument")]
    UnexpectedArg {
        #[label("here")]
        span: SourceSpan,
    },

    #[error("missing argument '{name}'")]
    MissingArg {
        name: &'static str,
        #[label("here")]
        span: SourceSpan,
    },

    #[error("unknown field '{name}'")]
    #[diagnostic(help("check the unit schema in README.md for valid field names"))]
    UnknownField {
        name: String,
        #[label("unknown")]
        span: SourceSpan,
    },

    #[error("duplicate field '{name}'")]
    DuplicateField {
        name: String,
        #[label("already defined above")]
        span: SourceSpan,
    },

    #[error("missing required field '{name}'")]
    #[diagnostic(help("add '{name}' to the unit config"))]
    MissingField {
        name: String,
        #[label("required here")]
        span: SourceSpan,
    },

    #[error("invalid field '{name}'")]
    InvalidField {
        name: String,
        #[label("in this field")]
        span: SourceSpan,
        #[diagnostic_source]
        #[source]
        source: FieldError,
    },

    #[error("unknown value '{value}' for field '{field}'")]
    UnknownVariant {
        field: String,
        value: String,
        #[label("not a valid {field}")]
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

#[derive(Debug, Error, Diagnostic)]
pub enum ConfigError {
    #[error("unit '{name}' not found")]
    #[diagnostic(help("run `dpl unit list` to see available units"))]
    NotFound { name: String },

    #[error("read config for unit '{name}'")]
    Read {
        name: String,
        #[source]
        source: io::Error,
    },

    #[error("write config for unit '{name}'")]
    Write {
        name: String,
        #[source]
        source: io::Error,
    },

    #[error("parse config for unit '{name}'")]
    Parse {
        name: String,
        #[diagnostic_source]
        #[source]
        source: kdl::KdlError,
    },

    #[error("invalid config for unit '{name}'")]
    Semantic {
        name: String,
        #[source_code]
        src: NamedSource<String>,
        #[diagnostic_source]
        #[source]
        source: Box<dyn Diagnostic + Send + Sync + 'static>,
    },
}
