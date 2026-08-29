//! Internal AST nodes used to preserve incomplete editor input.

use std::cell::Cell;

use oxc_allocator::{CloneIn, Dummy, ReplaceWith, TakeIn, UnstableAddress};
use oxc_ast_macros::ast;
use oxc_span::{ContentEq, GetSpan, GetSpanMut, Span};
use oxc_syntax::node::NodeId;

/// A zero-width placeholder for an expression omitted from incomplete editor input.
#[ast(visit)]
#[derive(Debug, Clone)]
#[generate_derive(CloneIn, Dummy, ReplaceWith, TakeIn)]
#[generate_derive(ContentEq, GetSpan, GetSpanMut, UnstableAddress)]
pub struct MissingExpression {
    pub node_id: Cell<NodeId>,
    pub span: Span,
}
