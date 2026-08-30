//! Internal AST nodes used to preserve incomplete editor input.

use std::cell::Cell;

use oxc_allocator::{CloneIn, Dummy, ReplaceWith, TakeIn, UnstableAddress};
use oxc_ast_macros::ast;
use oxc_span::{ContentEq, GetSpan, GetSpanMut, Span};
use oxc_syntax::node::NodeId;

use super::Expression;

/// A zero-width placeholder for an expression omitted from incomplete editor input.
#[ast(visit)]
#[derive(Debug, Clone)]
#[generate_derive(CloneIn, Dummy, ReplaceWith, TakeIn)]
#[generate_derive(ContentEq, GetSpan, GetSpanMut, UnstableAddress)]
pub struct MissingExpression {
    pub node_id: Cell<NodeId>,
    pub span: Span,
}

/// A source-backed expression token that could not form valid syntax during editor recovery.
#[ast(visit)]
#[derive(Debug, Clone)]
#[generate_derive(CloneIn, Dummy, ReplaceWith, TakeIn)]
#[generate_derive(ContentEq, GetSpan, GetSpanMut, UnstableAddress)]
pub struct MalformedExpression {
    pub node_id: Cell<NodeId>,
    pub span: Span,
}

/// A member expression whose property name is absent from incomplete editor input.
#[ast(visit)]
#[derive(Debug)]
#[generate_derive(CloneIn, Dummy, ReplaceWith, TakeIn)]
#[generate_derive(ContentEq, GetSpan, GetSpanMut, UnstableAddress)]
pub struct MissingMemberExpression<'a> {
    pub node_id: Cell<NodeId>,
    /// Full expression span, including the source-backed object and access operator.
    pub span: Span,
    pub object: Expression<'a>,
    /// Zero-width insertion point for the absent property identifier.
    pub missing_property_span: Span,
    pub optional: bool,
}

/// A zero-width placeholder for a type omitted from incomplete editor input.
#[ast(visit)]
#[derive(Debug, Clone)]
#[generate_derive(CloneIn, Dummy, ReplaceWith, TakeIn)]
#[generate_derive(ContentEq, GetSpan, GetSpanMut, UnstableAddress)]
pub struct MissingType {
    pub node_id: Cell<NodeId>,
    pub span: Span,
}
