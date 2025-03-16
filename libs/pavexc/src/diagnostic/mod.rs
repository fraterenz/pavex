//! A toolkit to assemble and report errors and warnings to the user.
pub(crate) use ordinals::ZeroBasedOrdinal;
pub(crate) use pavex_cli_diagnostic::{
    AnnotatedSource, CompilerDiagnostic, CompilerDiagnosticBuilder, HelpWithSnippet,
};
pub(crate) use proc_macro_utils::ProcMacroSpanExt;
pub(crate) use registration_locations::{
    f_macro_span, get_bp_new_span, get_config_key_span, get_domain_span, get_nest_blueprint_span,
    get_prefix_span, get_route_path_span,
};
pub(crate) use source_file::{LocationExt, ParsedSourceFile, read_source_file};

pub(crate) use self::miette::{
    LabeledSpanExt, OptionalLabeledSpanExt, OptionalSourceSpanExt, SourceSpanExt,
    convert_proc_macro_span, convert_rustdoc_span,
};
pub(crate) use callable_definition::CallableDefinition;
pub(crate) use kind::ComponentKind;
pub(crate) use sink::DiagnosticSink;

mod callable_definition;
mod kind;
mod miette;
mod ordinals;
mod proc_macro_utils;
mod registration_locations;
mod sink;
mod source_file;
