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

#[test]
fn missing_assignment_rhs_is_semantically_inert_and_later_bindings_survive() {
    let allocator = Allocator::default();
    let parsed = Parser::new(
        &allocator,
        "let target = 1; target = ; const intact: number = 1;",
        SourceType::ts(),
    )
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
    assert_eq!(binding_names, ["intact", "target"]);
    assert_eq!(semantic.scoping().references_len(), 1);

    let missing_nodes = semantic
        .nodes()
        .iter()
        .filter(|node| matches!(node.kind(), AstKind::MissingExpression(_)))
        .count();
    assert_eq!(missing_nodes, 1);
}

#[test]
fn missing_object_property_value_is_semantically_inert_and_later_nodes_survive() {
    let allocator = Allocator::default();
    let parsed = Parser::new(
        &allocator,
        "const value = { missing: , intact: 1 }; const later = 2;",
        SourceType::ts(),
    )
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
    assert_eq!(binding_names, ["later", "value"]);
    assert_eq!(semantic.scoping().references_len(), 0);

    let missing_nodes = semantic
        .nodes()
        .iter()
        .filter(|node| matches!(node.kind(), AstKind::MissingExpression(_)))
        .count();
    assert_eq!(missing_nodes, 1);
}

#[test]
fn missing_array_operands_are_semantically_inert_and_later_nodes_survive() {
    let allocator = Allocator::default();
    let parsed = Parser::new(
        &allocator,
        "let target = 1; const values = [target = , ... , 2]; const later = 3;",
        SourceType::ts(),
    )
    .with_options(ParseOptions { mode: ParseMode::Editor, ..ParseOptions::default() })
    .parse();

    assert!(!parsed.panicked);
    assert_eq!(parsed.diagnostics.len(), 2);

    let built = SemanticBuilder::new_compiler().with_build_nodes(true).build(&parsed.program);
    assert!(built.diagnostics.is_empty());

    let semantic = built.semantic;
    let root = semantic.scoping().root_scope_id();
    let mut binding_names =
        semantic.scoping().get_bindings(root).keys().map(Ident::as_str).collect::<Vec<_>>();
    binding_names.sort_unstable();
    assert_eq!(binding_names, ["later", "target", "values"]);
    assert_eq!(semantic.scoping().references_len(), 1);

    let missing_nodes = semantic
        .nodes()
        .iter()
        .filter(|node| matches!(node.kind(), AstKind::MissingExpression(_)))
        .count();
    assert_eq!(missing_nodes, 2);
}

#[test]
fn missing_call_arguments_are_semantically_inert_and_later_nodes_survive() {
    let allocator = Allocator::default();
    let parsed = Parser::new(
        &allocator,
        "let target = 1; function f(a: number, b: number, c: number): void {} f(target = , ... , 1, , 2); const later = 3;",
        SourceType::ts(),
    )
    .with_options(ParseOptions { mode: ParseMode::Editor, ..ParseOptions::default() })
    .parse();

    assert!(!parsed.panicked);
    assert_eq!(parsed.diagnostics.len(), 3);

    let built = SemanticBuilder::new_compiler().with_build_nodes(true).build(&parsed.program);
    assert!(built.diagnostics.is_empty());

    let semantic = built.semantic;
    let root = semantic.scoping().root_scope_id();
    let mut binding_names =
        semantic.scoping().get_bindings(root).keys().map(Ident::as_str).collect::<Vec<_>>();
    binding_names.sort_unstable();
    assert_eq!(binding_names, ["f", "later", "target"]);
    assert_eq!(semantic.scoping().references_len(), 2);

    let missing_nodes = semantic
        .nodes()
        .iter()
        .filter(|node| matches!(node.kind(), AstKind::MissingExpression(_)))
        .count();
    assert_eq!(missing_nodes, 3);
}

#[test]
fn missing_list_delimiters_preserve_semantic_bindings_and_references() {
    let allocator = Allocator::default();
    let parsed = Parser::new(
        &allocator,
        "function f(a: number, b: number): void {} const object = { first: 1 second: 2 }; const array = [1 2]; f(1 2); const later = 3;",
        SourceType::ts(),
    )
    .with_options(ParseOptions { mode: ParseMode::Editor, ..ParseOptions::default() })
    .parse();

    assert!(!parsed.panicked);
    assert_eq!(parsed.diagnostics.len(), 3);
    assert_eq!(parsed.recoveries.len(), 3);
    assert!(parsed.recoveries.iter().all(|recovery| recovery.kind == "MissingComma"));

    let built = SemanticBuilder::new_compiler().with_build_nodes(true).build(&parsed.program);
    assert!(built.diagnostics.is_empty());

    let semantic = built.semantic;
    let root = semantic.scoping().root_scope_id();
    let mut binding_names =
        semantic.scoping().get_bindings(root).keys().map(Ident::as_str).collect::<Vec<_>>();
    binding_names.sort_unstable();
    assert_eq!(binding_names, ["array", "f", "later", "object"]);
    assert_eq!(semantic.scoping().references_len(), 1);

    let missing_nodes = semantic
        .nodes()
        .iter()
        .filter(|node| matches!(node.kind(), AstKind::MissingExpression(_)))
        .count();
    assert_eq!(missing_nodes, 0);
}

#[test]
fn missing_list_closers_preserve_later_semantic_nodes() {
    for source_text in [
        "const objects = [{ first: 1]; const later = 2;",
        "const object = { values: [1, 2 }; const later = 3;",
        "function f(a: number, b: number): void {} const calls = [f(1, 2]; const later = 3;",
    ] {
        let allocator = Allocator::default();
        let parsed = Parser::new(&allocator, source_text, SourceType::ts())
            .with_options(ParseOptions { mode: ParseMode::Editor, ..ParseOptions::default() })
            .parse();

        assert!(!parsed.panicked, "editor mode should recover {source_text:?}");
        assert_eq!(parsed.diagnostics.len(), 1);
        assert_eq!(parsed.recoveries.len(), 1);

        let built = SemanticBuilder::new_compiler().with_build_nodes(true).build(&parsed.program);
        assert!(built.diagnostics.is_empty(), "semantic build should accept {source_text:?}");

        let semantic = built.semantic;
        let root = semantic.scoping().root_scope_id();
        let has_later_binding = semantic
            .scoping()
            .get_bindings(root)
            .keys()
            .map(Ident::as_str)
            .any(|name| name == "later");
        assert!(has_later_binding, "later binding should survive {source_text:?}");
    }
}

#[test]
fn missing_parameter_delimiters_preserve_function_and_later_bindings() {
    let allocator = Allocator::default();
    let parsed = Parser::new(
        &allocator,
        "function f(first: number second: string): void {} const later = 3;",
        SourceType::ts(),
    )
    .with_options(ParseOptions { mode: ParseMode::Editor, ..ParseOptions::default() })
    .parse();

    assert!(!parsed.panicked);
    assert_eq!(parsed.diagnostics.len(), 1);
    assert_eq!(parsed.recoveries.len(), 1);
    assert_eq!(parsed.recoveries[0].kind, "MissingComma");

    let built = SemanticBuilder::new_compiler().with_build_nodes(true).build(&parsed.program);
    assert!(built.diagnostics.is_empty());

    let semantic = built.semantic;
    let root = semantic.scoping().root_scope_id();
    let mut binding_names =
        semantic.scoping().get_bindings(root).keys().map(Ident::as_str).collect::<Vec<_>>();
    binding_names.sort_unstable();
    assert_eq!(binding_names, ["f", "later"]);
}

#[test]
fn missing_parameter_does_not_invent_a_semantic_binding() {
    let allocator = Allocator::default();
    let parsed = Parser::new(
        &allocator,
        "function broken(, second: number): void {} const later = 3;",
        SourceType::ts(),
    )
    .with_options(ParseOptions { mode: ParseMode::Editor, ..ParseOptions::default() })
    .parse();

    assert!(!parsed.panicked);
    assert_eq!(parsed.diagnostics.len(), 1);
    assert_eq!(parsed.recoveries.len(), 1);
    assert_eq!(parsed.recoveries[0].kind, "MissingParameter");

    let built = SemanticBuilder::new_compiler().with_build_nodes(true).build(&parsed.program);
    assert!(built.diagnostics.is_empty());
    let semantic = built.semantic;
    let root = semantic.scoping().root_scope_id();
    let mut binding_names =
        semantic.scoping().get_bindings(root).keys().map(Ident::as_str).collect::<Vec<_>>();
    binding_names.sort_unstable();
    assert_eq!(binding_names, ["broken", "later"]);
}

#[test]
fn missing_function_body_closer_preserves_function_and_prior_bindings() {
    let allocator = Allocator::default();
    let parsed = Parser::new(
        &allocator,
        "const prior = 1; function f(): number { return prior;",
        SourceType::ts(),
    )
    .with_options(ParseOptions { mode: ParseMode::Editor, ..ParseOptions::default() })
    .parse();

    assert!(!parsed.panicked);
    assert_eq!(parsed.diagnostics.len(), 1);
    assert_eq!(parsed.recoveries.len(), 1);
    assert_eq!(parsed.recoveries[0].kind, "MissingClosingBrace");

    let built = SemanticBuilder::new_compiler().with_build_nodes(true).build(&parsed.program);
    assert!(built.diagnostics.is_empty());

    let semantic = built.semantic;
    let root = semantic.scoping().root_scope_id();
    let mut binding_names =
        semantic.scoping().get_bindings(root).keys().map(Ident::as_str).collect::<Vec<_>>();
    binding_names.sort_unstable();
    assert_eq!(binding_names, ["f", "prior"]);
    assert_eq!(semantic.scoping().references_len(), 1);
}

#[test]
fn missing_return_expression_operand_is_semantically_inert() {
    let allocator = Allocator::default();
    let parsed = Parser::new(
        &allocator,
        "function f(): number { return 1 + } const later = 2;",
        SourceType::ts(),
    )
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
    assert_eq!(binding_names, ["f", "later"]);
    let missing_nodes = semantic
        .nodes()
        .iter()
        .filter(|node| matches!(node.kind(), AstKind::MissingExpression(_)))
        .count();
    assert_eq!(missing_nodes, 1);
}

#[test]
fn missing_interface_closer_preserves_interface_and_prior_bindings() {
    let allocator = Allocator::default();
    let parsed = Parser::new(
        &allocator,
        "const prior = 1; interface Box { value: number;",
        SourceType::ts(),
    )
    .with_options(ParseOptions { mode: ParseMode::Editor, ..ParseOptions::default() })
    .parse();

    assert!(!parsed.panicked);
    assert_eq!(parsed.diagnostics.len(), 1);
    assert_eq!(parsed.recoveries.len(), 1);
    assert_eq!(parsed.recoveries[0].kind, "MissingClosingBrace");

    let built = SemanticBuilder::new_compiler().with_build_nodes(true).build(&parsed.program);
    assert!(built.diagnostics.is_empty());
    let semantic = built.semantic;
    let root = semantic.scoping().root_scope_id();
    let mut binding_names =
        semantic.scoping().get_bindings(root).keys().map(Ident::as_str).collect::<Vec<_>>();
    binding_names.sort_unstable();
    assert_eq!(binding_names, ["Box", "prior"]);
}

#[test]
fn missing_member_name_preserves_object_reference_and_later_bindings() {
    let allocator = Allocator::default();
    let parsed = Parser::new(
        &allocator,
        "interface Box { value: number; } declare const box: Box; box.; const later = 2;",
        SourceType::ts(),
    )
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
    assert_eq!(binding_names, ["Box", "box", "later"]);
    assert_eq!(semantic.scoping().references_len(), 2);

    let missing_nodes = semantic
        .nodes()
        .iter()
        .filter(|node| matches!(node.kind(), AstKind::MissingMemberExpression(_)))
        .count();
    assert_eq!(missing_nodes, 1);
}

#[test]
fn missing_types_are_semantically_inert_and_later_bindings_survive() {
    let allocator = Allocator::default();
    let parsed = Parser::new(
        &allocator,
        "type Broken = ; const value: = 1; const intact: number = 2;",
        SourceType::ts(),
    )
    .with_options(ParseOptions { mode: ParseMode::Editor, ..ParseOptions::default() })
    .parse();

    assert!(!parsed.panicked);
    assert_eq!(parsed.diagnostics.len(), 2);

    let built = SemanticBuilder::new_compiler().with_build_nodes(true).build(&parsed.program);
    assert!(built.diagnostics.is_empty());

    let semantic = built.semantic;
    let root = semantic.scoping().root_scope_id();
    let binding_names =
        semantic.scoping().get_bindings(root).keys().map(Ident::as_str).collect::<Vec<_>>();
    assert!(binding_names.contains(&"value"));
    assert!(binding_names.contains(&"intact"));
    assert_eq!(semantic.scoping().references_len(), 0);

    let missing_nodes = semantic
        .nodes()
        .iter()
        .filter(|node| matches!(node.kind(), AstKind::MissingType(_)))
        .count();
    assert_eq!(missing_nodes, 2);
}

#[test]
fn malformed_expressions_are_semantically_inert_and_later_bindings_survive() {
    let allocator = Allocator::default();
    let parsed = Parser::new(
        &allocator,
        "const broken: number = :; let target = 1; target = ...; const intact: number = 2;",
        SourceType::ts(),
    )
    .with_options(ParseOptions { mode: ParseMode::Editor, ..ParseOptions::default() })
    .parse();

    assert!(!parsed.panicked);
    assert_eq!(parsed.diagnostics.len(), 2);

    let built = SemanticBuilder::new_compiler().with_build_nodes(true).build(&parsed.program);
    assert!(built.diagnostics.is_empty());

    let semantic = built.semantic;
    let root = semantic.scoping().root_scope_id();
    let mut binding_names =
        semantic.scoping().get_bindings(root).keys().map(Ident::as_str).collect::<Vec<_>>();
    binding_names.sort_unstable();
    assert_eq!(binding_names, ["broken", "intact", "target"]);
    assert_eq!(semantic.scoping().references_len(), 1);

    let malformed_nodes = semantic
        .nodes()
        .iter()
        .filter(|node| matches!(node.kind(), AstKind::MalformedExpression(_)))
        .count();
    assert_eq!(malformed_nodes, 2);
}

#[test]
fn missing_class_member_separator_preserves_class_and_later_bindings() {
    let allocator = Allocator::default();
    let parsed = Parser::new(
        &allocator,
        "class Box { first: number = 1 second: string = \"ok\"; } const later = 2;",
        SourceType::ts(),
    )
    .with_options(ParseOptions { mode: ParseMode::Editor, ..ParseOptions::default() })
    .parse();

    assert!(!parsed.panicked);
    assert_eq!(parsed.diagnostics.len(), 1);
    assert_eq!(parsed.recoveries.len(), 1);
    assert_eq!(parsed.recoveries[0].kind, "MissingSemicolon");

    let built = SemanticBuilder::new_compiler().with_build_nodes(true).build(&parsed.program);
    assert!(built.diagnostics.is_empty());
    let semantic = built.semantic;
    let root = semantic.scoping().root_scope_id();
    let mut binding_names =
        semantic.scoping().get_bindings(root).keys().map(Ident::as_str).collect::<Vec<_>>();
    binding_names.sort_unstable();
    assert_eq!(binding_names, ["Box", "later"]);
}

#[test]
fn missing_class_closer_preserves_class_and_prior_bindings() {
    let allocator = Allocator::default();
    let parsed = Parser::new(
        &allocator,
        "const prior = 1; class Box { value: number = 1;",
        SourceType::ts(),
    )
    .with_options(ParseOptions { mode: ParseMode::Editor, ..ParseOptions::default() })
    .parse();

    assert!(!parsed.panicked);
    assert_eq!(parsed.diagnostics.len(), 1);
    assert_eq!(parsed.recoveries.len(), 1);
    assert_eq!(parsed.recoveries[0].kind, "MissingClosingBrace");

    let built = SemanticBuilder::new_compiler().with_build_nodes(true).build(&parsed.program);
    assert!(built.diagnostics.is_empty());
    let semantic = built.semantic;
    let root = semantic.scoping().root_scope_id();
    let mut binding_names =
        semantic.scoping().get_bindings(root).keys().map(Ident::as_str).collect::<Vec<_>>();
    binding_names.sort_unstable();
    assert_eq!(binding_names, ["Box", "prior"]);
}

#[test]
fn missing_call_closer_preserves_the_following_declaration_binding() {
    let allocator = Allocator::default();
    let parsed = Parser::new(
        &allocator,
        "function check(value: number): void {}\ncheck(\nconst later = 2;",
        SourceType::ts(),
    )
    .with_options(ParseOptions { mode: ParseMode::Editor, ..ParseOptions::default() })
    .parse();

    assert!(!parsed.panicked);
    assert_eq!(parsed.diagnostics.len(), 1);
    assert_eq!(parsed.recoveries.len(), 1);
    assert_eq!(parsed.recoveries[0].kind, "MissingClosingParenthesis");

    let built = SemanticBuilder::new_compiler().with_build_nodes(true).build(&parsed.program);
    assert!(built.diagnostics.is_empty());
    let semantic = built.semantic;
    let root = semantic.scoping().root_scope_id();
    let mut binding_names =
        semantic.scoping().get_bindings(root).keys().map(Ident::as_str).collect::<Vec<_>>();
    binding_names.sort_unstable();
    assert_eq!(binding_names, ["check", "later"]);
}

#[test]
fn missing_variable_declaration_name_does_not_invent_a_binding() {
    let allocator = Allocator::default();
    let parsed = Parser::new(&allocator, "const = 1;\nconst later = 2;", SourceType::ts())
        .with_options(ParseOptions { mode: ParseMode::Editor, ..ParseOptions::default() })
        .parse();

    assert!(!parsed.panicked);
    assert_eq!(parsed.diagnostics.len(), 2);
    assert_eq!(parsed.recoveries.len(), 2);

    let built = SemanticBuilder::new_compiler().with_build_nodes(true).build(&parsed.program);
    assert!(built.diagnostics.is_empty());
    let semantic = built.semantic;
    let root = semantic.scoping().root_scope_id();
    let binding_names =
        semantic.scoping().get_bindings(root).keys().map(Ident::as_str).collect::<Vec<_>>();
    assert_eq!(binding_names, ["later"]);
}
