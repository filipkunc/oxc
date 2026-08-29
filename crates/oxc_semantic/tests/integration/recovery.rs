use oxc_allocator::Allocator;
use oxc_ast::{AstKind, ast::Ident};
use oxc_parser::{ParseMode, ParseOptions, Parser};
use oxc_semantic::SemanticBuilder;
use oxc_span::SourceType;

#[test]
fn missing_expression_is_semantically_inert_and_later_bindings_survive() {
    let allocator = Allocator::default();
    let parsed =
        Parser::new(&allocator, "const broken = ; const intact: number = 1;", SourceType::ts())
            .with_options(ParseOptions { mode: ParseMode::Editor, ..ParseOptions::default() })
            .parse();

    assert!(!parsed.panicked);
    assert_eq!(parsed.diagnostics.len(), 1);

    let built = SemanticBuilder::new_compiler().with_build_nodes(true).build(&parsed.program);
    assert!(built.diagnostics.is_empty());

    let semantic = built.semantic;
    let root = semantic.scoping().root_scope_id();
    let mut binding_names =
        semantic.scoping().get_bindings(root).keys().map(Ident::as_str).collect::<Vec<_>>();
    binding_names.sort_unstable();
    assert_eq!(binding_names, ["broken", "intact"]);
    assert_eq!(semantic.scoping().references_len(), 0);

    let missing_nodes = semantic
        .nodes()
        .iter()
        .filter(|node| matches!(node.kind(), AstKind::MissingExpression(_)))
        .count();
    assert_eq!(missing_nodes, 1);
}
