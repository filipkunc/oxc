use std::mem::size_of;

use oxc_allocator::Allocator;
use oxc_ast::{
    AstKind,
    ast::{Expression, TSType},
};
use oxc_ast_visit::{Visit, walk};
use oxc_parser::{ParseMode, ParseOptions, Parser};
use oxc_span::{GetSpan, SourceType, Span};

struct BoundedSpans {
    source_len: u32,
    node_count: usize,
    recovery_count: usize,
}

impl<'a> Visit<'a> for BoundedSpans {
    fn enter_node(&mut self, kind: AstKind<'a>) {
        let span = kind.span();
        assert!(span.start <= span.end, "reversed span for {kind:?}");
        assert!(span.end <= self.source_len, "out-of-bounds span for {kind:?}");
        self.node_count += 1;
        if matches!(
            kind,
            AstKind::MissingExpression(_)
                | AstKind::MalformedExpression(_)
                | AstKind::MissingMemberExpression(_)
                | AstKind::MissingType(_)
        ) {
            self.recovery_count += 1;
        }
    }
}

fn delete_first(source: &str, deleted: &str) -> String {
    let start = source.find(deleted).expect("deleted token exists in mutation seed");
    let end = start + deleted.len();
    let mut mutated = String::with_capacity(source.len() - deleted.len());
    mutated.push_str(&source[..start]);
    mutated.push_str(&source[end..]);
    mutated
}

#[derive(Default)]
struct MissingExpressions {
    spans: Vec<Span>,
    ast_kind_count: usize,
}

#[derive(Default)]
struct MissingTypes {
    spans: Vec<Span>,
    ast_kind_count: usize,
}

#[derive(Default)]
struct MalformedExpressions {
    spans: Vec<Span>,
    ast_kind_count: usize,
}

#[derive(Default)]
struct MissingMemberExpressions {
    spans: Vec<Span>,
    missing_property_spans: Vec<Span>,
    ast_kind_count: usize,
}

impl<'a> Visit<'a> for MissingMemberExpressions {
    fn enter_node(&mut self, kind: AstKind<'a>) {
        if matches!(kind, AstKind::MissingMemberExpression(_)) {
            self.ast_kind_count += 1;
        }
    }

    fn visit_missing_member_expression(
        &mut self,
        expression: &oxc_ast::ast::MissingMemberExpression<'a>,
    ) {
        self.spans.push(expression.span);
        self.missing_property_spans.push(expression.missing_property_span);
        walk::walk_missing_member_expression(self, expression);
    }
}

impl<'a> Visit<'a> for MalformedExpressions {
    fn enter_node(&mut self, kind: AstKind<'a>) {
        if matches!(kind, AstKind::MalformedExpression(_)) {
            self.ast_kind_count += 1;
        }
    }

    fn visit_malformed_expression(&mut self, expression: &oxc_ast::ast::MalformedExpression) {
        self.spans.push(expression.span);
        walk::walk_malformed_expression(self, expression);
    }
}

impl<'a> Visit<'a> for MissingTypes {
    fn enter_node(&mut self, kind: AstKind<'a>) {
        if matches!(kind, AstKind::MissingType(_)) {
            self.ast_kind_count += 1;
        }
    }

    fn visit_missing_type(&mut self, ty: &oxc_ast::ast::MissingType) {
        self.spans.push(ty.span);
        walk::walk_missing_type(self, ty);
    }
}

impl<'a> Visit<'a> for MissingExpressions {
    fn enter_node(&mut self, kind: AstKind<'a>) {
        if matches!(kind, AstKind::MissingExpression(_)) {
            self.ast_kind_count += 1;
        }
    }

    fn visit_missing_expression(&mut self, expression: &oxc_ast::ast::MissingExpression) {
        self.spans.push(expression.span);
        walk::walk_missing_expression(self, expression);
    }
}

fn assert_editor_missing_expression(source_text: &str, offset: u32) {
    let allocator = Allocator::default();
    let parsed = Parser::new(&allocator, source_text, SourceType::ts())
        .with_options(ParseOptions { mode: ParseMode::Editor, ..ParseOptions::default() })
        .parse();

    assert!(!parsed.panicked, "editor recovery should preserve the program");
    assert_eq!(parsed.diagnostics.len(), 1);
    assert_eq!(parsed.diagnostics[0].message, "Expression expected.");
    assert_eq!(parsed.diagnostics[0].code.scope.as_deref(), Some("TS"));
    assert_eq!(parsed.diagnostics[0].code.number.as_deref(), Some("1109"));

    let mut missing = MissingExpressions::default();
    missing.visit_program(&parsed.program);
    assert_eq!(missing.spans, [Span::empty(offset)]);
    assert_eq!(missing.ast_kind_count, 1);
}

#[test]
fn normal_mode_remains_fatal_for_a_missing_variable_initializer() {
    for source_text in [
        "let value =",
        "const broken =\nconst intact: number = 1;",
        "const broken = const intact: number = 1;",
    ] {
        let allocator = Allocator::default();
        let parsed = Parser::new(&allocator, source_text, SourceType::ts()).parse();

        assert!(parsed.panicked);
        assert!(parsed.program.body.is_empty());
        assert!(!parsed.diagnostics.is_empty());
    }
}

#[test]
fn editor_mode_recovers_at_supported_initializer_boundaries() {
    for (source_text, offset) in [
        ("let value =", 11),
        ("const value = ;", 14),
        ("const first =, second = 2;", 13),
        ("function f() { const local = }", 29),
    ] {
        assert_editor_missing_expression(source_text, offset);
    }
}

#[test]
fn editor_mode_does_not_invent_an_expression_without_an_equals_token() {
    for source_text in ["let", "let value"] {
        let allocator = Allocator::default();
        let parsed = Parser::new(&allocator, source_text, SourceType::ts())
            .with_options(ParseOptions { mode: ParseMode::Editor, ..ParseOptions::default() })
            .parse();
        let mut missing = MissingExpressions::default();
        missing.visit_program(&parsed.program);
        assert!(missing.spans.is_empty());
    }
}

#[test]
fn editor_mode_preserves_the_boundary_and_following_declarations() {
    let allocator = Allocator::default();
    let parsed =
        Parser::new(&allocator, "const broken = ; const intact: number = 1;", SourceType::ts())
            .with_options(ParseOptions { mode: ParseMode::Editor, ..ParseOptions::default() })
            .parse();

    assert!(!parsed.panicked);
    assert_eq!(parsed.program.body.len(), 2);

    let oxc_ast::ast::Statement::VariableDeclaration(first) = &parsed.program.body[0] else {
        panic!("expected a variable declaration");
    };
    assert!(matches!(first.declarations[0].init, Some(Expression::MissingExpression(_))));

    let oxc_ast::ast::Statement::VariableDeclaration(second) = &parsed.program.body[1] else {
        panic!("expected a variable declaration");
    };
    assert!(matches!(second.declarations[0].init, Some(Expression::NumericLiteral(_))));
}

#[test]
fn editor_mode_resumes_at_a_following_variable_statement_without_a_semicolon() {
    for source_text in
        ["const broken =\nconst intact: number = 1;", "const broken = const intact: number = 1;"]
    {
        let allocator = Allocator::default();
        let parsed = Parser::new(&allocator, source_text, SourceType::ts())
            .with_options(ParseOptions { mode: ParseMode::Editor, ..ParseOptions::default() })
            .parse();

        assert!(!parsed.panicked, "editor recovery should preserve the program");
        assert_eq!(parsed.program.body.len(), 2);
        assert_eq!(parsed.diagnostics.len(), 1);
        assert_eq!(parsed.diagnostics[0].message, "Expression expected.");

        let oxc_ast::ast::Statement::VariableDeclaration(first) = &parsed.program.body[0] else {
            panic!("expected the recovered variable declaration");
        };
        let Some(Expression::MissingExpression(missing)) = &first.declarations[0].init else {
            panic!("expected a missing initializer");
        };
        assert_eq!(missing.span, Span::empty(15));

        let oxc_ast::ast::Statement::VariableDeclaration(second) = &parsed.program.body[1] else {
            panic!("expected the following variable declaration");
        };
        assert!(matches!(second.declarations[0].init, Some(Expression::NumericLiteral(_))));
    }
}

#[test]
fn editor_mode_resumes_at_a_var_statement_inside_a_block() {
    let source_text = "function f() { const broken = var intact: number = 1; }";
    let boundary = u32::try_from(source_text.find("var intact").expect("following declaration"))
        .expect("source offset fits in u32");
    let allocator = Allocator::default();
    let parsed = Parser::new(&allocator, source_text, SourceType::ts())
        .with_options(ParseOptions { mode: ParseMode::Editor, ..ParseOptions::default() })
        .parse();

    assert!(!parsed.panicked, "editor recovery should preserve the function");
    assert_eq!(parsed.diagnostics.len(), 1);
    let mut missing = MissingExpressions::default();
    missing.visit_program(&parsed.program);
    assert_eq!(missing.spans, [Span::empty(boundary)]);

    let oxc_ast::ast::Statement::FunctionDeclaration(function) = &parsed.program.body[0] else {
        panic!("expected a function declaration");
    };
    let body = function.body.as_ref().expect("function body");
    assert_eq!(body.statements.len(), 2);
}

#[test]
fn normal_mode_remains_fatal_for_a_missing_assignment_rhs() {
    let allocator = Allocator::default();
    let parsed = Parser::new(&allocator, "let value = 1; value =", SourceType::ts()).parse();

    assert!(parsed.panicked);
    assert!(parsed.program.body.is_empty());
    assert!(!parsed.diagnostics.is_empty());
}

#[test]
fn normal_mode_remains_fatal_for_a_missing_object_property_value() {
    let allocator = Allocator::default();
    let parsed =
        Parser::new(&allocator, "const value = { missing: , intact: 1 };", SourceType::ts())
            .parse();

    assert!(parsed.panicked);
    assert!(parsed.program.body.is_empty());
    assert!(!parsed.diagnostics.is_empty());
}

#[test]
fn editor_mode_recovers_missing_object_property_values_at_owned_boundaries() {
    for source_text in [
        "const value = { missing: , intact: 1 };",
        "const value = { missing: } ; const intact = 1;",
        "const value = { nested: { missing: , intact: 1 }, after: 2 };",
        "let target = 1; const value = { missing: target = , intact: 1 };",
    ] {
        let offset = u32::try_from(
            source_text
                .find(", intact")
                .or_else(|| source_text.find("} ;"))
                .expect("test has a missing-value boundary"),
        )
        .expect("test source offset fits in u32");
        assert_editor_missing_expression(source_text, offset);
    }
}

#[test]
fn editor_mode_puts_the_missing_expression_on_the_object_property_value() {
    let allocator = Allocator::default();
    let parsed =
        Parser::new(&allocator, "const value = { missing: , intact: 1 };", SourceType::ts())
            .with_options(ParseOptions { mode: ParseMode::Editor, ..ParseOptions::default() })
            .parse();

    assert!(!parsed.panicked);
    let oxc_ast::ast::Statement::VariableDeclaration(declaration) = &parsed.program.body[0] else {
        panic!("expected a variable declaration");
    };
    let Some(Expression::ObjectExpression(object)) = &declaration.declarations[0].init else {
        panic!("expected an object initializer");
    };
    let oxc_ast::ast::ObjectPropertyKind::ObjectProperty(missing) = &object.properties[0] else {
        panic!("expected an object property");
    };
    assert!(matches!(missing.value, Expression::MissingExpression(_)));
    let oxc_ast::ast::ObjectPropertyKind::ObjectProperty(intact) = &object.properties[1] else {
        panic!("expected an object property");
    };
    assert!(matches!(intact.value, Expression::NumericLiteral(_)));
}

#[test]
fn editor_mode_object_property_recovery_is_deterministic_and_makes_progress() {
    let source_text = "const value = { first: , second: , intact: 1 }; const later = 2;";
    let parse = || {
        let allocator = Allocator::default();
        let parsed = Parser::new(&allocator, source_text, SourceType::ts())
            .with_options(ParseOptions { mode: ParseMode::Editor, ..ParseOptions::default() })
            .parse();
        assert!(!parsed.panicked);
        assert_eq!(parsed.program.body.len(), 2);
        assert_eq!(parsed.diagnostics.len(), 2);

        let mut missing = MissingExpressions::default();
        missing.visit_program(&parsed.program);
        assert_eq!(missing.spans, [Span::empty(23), Span::empty(33)]);
        assert_eq!(missing.ast_kind_count, 2);

        (
            format!("{:#?}", parsed.program),
            parsed
                .diagnostics
                .iter()
                .map(|diagnostic| (diagnostic.message.to_string(), diagnostic.labels.clone()))
                .collect::<Vec<_>>(),
        )
    };

    assert_eq!(parse(), parse());
}

#[test]
fn editor_mode_does_not_change_valid_object_input() {
    let source_text = "const value = { first: 1, nested: { second: 2 } };";
    let normal_allocator = Allocator::default();
    let normal = Parser::new(&normal_allocator, source_text, SourceType::ts()).parse();
    let editor_allocator = Allocator::default();
    let editor = Parser::new(&editor_allocator, source_text, SourceType::ts())
        .with_options(ParseOptions { mode: ParseMode::Editor, ..ParseOptions::default() })
        .parse();

    assert!(normal.diagnostics.is_empty());
    assert!(editor.diagnostics.is_empty());
    assert_eq!(format!("{:#?}", normal.program), format!("{:#?}", editor.program));
}

#[test]
fn normal_mode_remains_fatal_for_missing_array_operands() {
    for source_text in
        ["let target = 1; const values = [target = , 2];", "const values = [... , 2];"]
    {
        let allocator = Allocator::default();
        let parsed = Parser::new(&allocator, source_text, SourceType::ts()).parse();
        assert!(parsed.panicked);
        assert!(parsed.program.body.is_empty());
        assert!(!parsed.diagnostics.is_empty());
    }
}

#[test]
fn editor_mode_recovers_missing_array_assignment_and_spread_operands() {
    for source_text in [
        "let target = 1; const values = [target = , 2];",
        "let target = 1; const values = [target = ];",
        "const values = [... , 2];",
        "const values = [... ];",
        "const values = [{ nested: [... , 2] }, 3];",
    ] {
        let offset = u32::try_from(
            source_text
                .find(", 2")
                .or_else(|| source_text.find(']'))
                .expect("test has an array recovery boundary"),
        )
        .expect("test source offset fits in u32");
        assert_editor_missing_expression(source_text, offset);
    }
}

#[test]
fn editor_mode_preserves_array_holes_without_recovery_nodes() {
    let source_text = "const values = [, 1, , 2,];";
    let allocator = Allocator::default();
    let parsed = Parser::new(&allocator, source_text, SourceType::ts())
        .with_options(ParseOptions { mode: ParseMode::Editor, ..ParseOptions::default() })
        .parse();

    assert!(!parsed.panicked);
    assert!(parsed.diagnostics.is_empty());
    let mut missing = MissingExpressions::default();
    missing.visit_program(&parsed.program);
    assert!(missing.spans.is_empty());
}

#[test]
fn editor_mode_array_recovery_is_deterministic_and_makes_progress() {
    let source_text = "let target = 1; const values = [target = , ... , 2]; const later = 3;";
    let parse = || {
        let allocator = Allocator::default();
        let parsed = Parser::new(&allocator, source_text, SourceType::ts())
            .with_options(ParseOptions { mode: ParseMode::Editor, ..ParseOptions::default() })
            .parse();
        assert!(!parsed.panicked);
        assert_eq!(parsed.program.body.len(), 3);
        assert_eq!(parsed.diagnostics.len(), 2);

        let mut missing = MissingExpressions::default();
        missing.visit_program(&parsed.program);
        assert_eq!(missing.spans, [Span::empty(41), Span::empty(47)]);
        assert_eq!(missing.ast_kind_count, 2);

        (
            format!("{:#?}", parsed.program),
            parsed
                .diagnostics
                .iter()
                .map(|diagnostic| (diagnostic.message.to_string(), diagnostic.labels.clone()))
                .collect::<Vec<_>>(),
        )
    };

    assert_eq!(parse(), parse());
}

#[test]
fn editor_mode_does_not_change_valid_array_input() {
    let source_text = "const values = [1, ...[2, 3], { nested: [4] }];";
    let normal_allocator = Allocator::default();
    let normal = Parser::new(&normal_allocator, source_text, SourceType::ts()).parse();
    let editor_allocator = Allocator::default();
    let editor = Parser::new(&editor_allocator, source_text, SourceType::ts())
        .with_options(ParseOptions { mode: ParseMode::Editor, ..ParseOptions::default() })
        .parse();

    assert!(normal.diagnostics.is_empty());
    assert!(editor.diagnostics.is_empty());
    assert_eq!(format!("{:#?}", normal.program), format!("{:#?}", editor.program));
}

#[test]
fn normal_mode_remains_fatal_for_missing_call_arguments() {
    for source_text in ["f(target = , 2);", "f(... , 2);", "f(1, , 2);"] {
        let allocator = Allocator::default();
        let parsed = Parser::new(&allocator, source_text, SourceType::ts()).parse();
        assert!(parsed.panicked);
        assert!(parsed.program.body.is_empty());
        assert!(!parsed.diagnostics.is_empty());
    }
}

#[test]
fn editor_mode_recovers_missing_call_argument_operands() {
    for (source_text, expected_message) in [
        ("f(target = , 2); const later = 3;", "Expression expected."),
        ("f(target = ); const later = 3;", "Expression expected."),
        ("f(... , 2); const later = 3;", "Expression expected."),
        ("f(... ); const later = 3;", "Expression expected."),
        ("f(1, , 2); const later = 3;", "Argument expression expected."),
    ] {
        let allocator = Allocator::default();
        let parsed = Parser::new(&allocator, source_text, SourceType::ts())
            .with_options(ParseOptions { mode: ParseMode::Editor, ..ParseOptions::default() })
            .parse();

        assert!(!parsed.panicked);
        assert_eq!(parsed.program.body.len(), 2);
        assert_eq!(parsed.diagnostics.len(), 1);
        assert_eq!(parsed.diagnostics[0].message, expected_message);

        let mut missing = MissingExpressions::default();
        missing.visit_program(&parsed.program);
        assert_eq!(missing.spans.len(), 1);
        assert_eq!(missing.ast_kind_count, 1);
    }
}

#[test]
fn editor_mode_call_argument_recovery_is_deterministic_and_makes_progress() {
    let source_text = "f(target = , ... , 1, , 2); const later = 3;";
    let parse = || {
        let allocator = Allocator::default();
        let parsed = Parser::new(&allocator, source_text, SourceType::ts())
            .with_options(ParseOptions { mode: ParseMode::Editor, ..ParseOptions::default() })
            .parse();
        assert!(!parsed.panicked);
        assert_eq!(parsed.program.body.len(), 2);
        assert_eq!(parsed.diagnostics.len(), 3);

        let mut missing = MissingExpressions::default();
        missing.visit_program(&parsed.program);
        assert_eq!(missing.spans, [Span::empty(11), Span::empty(17), Span::empty(22)]);
        assert_eq!(missing.ast_kind_count, 3);

        (
            format!("{:#?}", parsed.program),
            parsed
                .diagnostics
                .iter()
                .map(|diagnostic| (diagnostic.message.to_string(), diagnostic.labels.clone()))
                .collect::<Vec<_>>(),
        )
    };

    assert_eq!(parse(), parse());
}

#[test]
fn editor_mode_does_not_change_valid_call_arguments() {
    let source_text = "f(1, ...values, { nested: [2] },);";
    let normal_allocator = Allocator::default();
    let normal = Parser::new(&normal_allocator, source_text, SourceType::ts()).parse();
    let editor_allocator = Allocator::default();
    let editor = Parser::new(&editor_allocator, source_text, SourceType::ts())
        .with_options(ParseOptions { mode: ParseMode::Editor, ..ParseOptions::default() })
        .parse();

    assert!(normal.diagnostics.is_empty());
    assert!(editor.diagnostics.is_empty());
    assert_eq!(format!("{:#?}", normal.program), format!("{:#?}", editor.program));
}

#[test]
fn normal_mode_remains_fatal_for_missing_list_delimiters() {
    for source_text in [
        "const value = { first: 1 second: 2 };",
        "const values = [1 2, 3];",
        "f(1 2, 3);",
        "const value = { first: 1",
        "const values = [1, 2",
        "f(1, 2",
    ] {
        let allocator = Allocator::default();
        let parsed = Parser::new(&allocator, source_text, SourceType::ts()).parse();
        assert!(parsed.panicked, "normal mode should abort for {source_text:?}");
        assert!(parsed.program.body.is_empty());
        assert!(parsed.recoveries.is_empty());
    }
}

#[test]
fn editor_mode_recovers_missing_object_array_and_call_commas() {
    for (source_text, offset) in [
        ("const value = { first: 1 second: 2 }; const later = 3;", 25),
        ("const values = [1 2, 3]; const later = 4;", 18),
        ("f(1 2, 3); const later = 4;", 4),
    ] {
        let allocator = Allocator::default();
        let parsed = Parser::new(&allocator, source_text, SourceType::ts())
            .with_options(ParseOptions { mode: ParseMode::Editor, ..ParseOptions::default() })
            .parse();

        assert!(!parsed.panicked, "editor mode should recover {source_text:?}");
        assert_eq!(parsed.program.body.len(), 2);
        assert_eq!(parsed.diagnostics.len(), 1);
        assert_eq!(parsed.diagnostics[0].message, "',' expected.");
        assert_eq!(parsed.recoveries.len(), 1);
        assert_eq!(parsed.recoveries[0].kind, "MissingComma");
        assert_eq!(parsed.recoveries[0].span, Span::empty(offset));

        let mut missing = MissingExpressions::default();
        missing.visit_program(&parsed.program);
        assert!(missing.spans.is_empty());
    }
}

#[test]
fn editor_mode_recovers_missing_list_closers_at_eof() {
    for (source_text, expected_kind, expected_offset) in [
        ("const value = { first: 1", "MissingClosingBrace", 24),
        ("const values = [1, 2", "MissingClosingBracket", 20),
        ("f(1, 2", "MissingClosingParenthesis", 6),
    ] {
        let allocator = Allocator::default();
        let parsed = Parser::new(&allocator, source_text, SourceType::ts())
            .with_options(ParseOptions { mode: ParseMode::Editor, ..ParseOptions::default() })
            .parse();

        assert!(!parsed.panicked, "editor mode should recover {source_text:?}");
        assert_eq!(parsed.program.body.len(), 1);
        assert_eq!(parsed.diagnostics.len(), 1);
        assert_eq!(parsed.recoveries.len(), 1);
        assert_eq!(parsed.recoveries[0].kind, expected_kind);
        assert_eq!(parsed.recoveries[0].span, Span::empty(expected_offset));
    }
}

#[test]
fn editor_mode_recovers_mismatched_closers_owned_by_outer_lists() {
    for (source_text, expected_kind) in [
        ("const value = [{ first: 1]; const later = 2;", "MissingClosingBrace"),
        ("const value = { nested: [1, 2 }; const later = 3;", "MissingClosingBracket"),
        ("f([1, 2); const later = 3;", "MissingClosingBracket"),
        ("const value = [f(1, 2]; const later = 3;", "MissingClosingParenthesis"),
    ] {
        let allocator = Allocator::default();
        let parsed = Parser::new(&allocator, source_text, SourceType::ts())
            .with_options(ParseOptions { mode: ParseMode::Editor, ..ParseOptions::default() })
            .parse();

        assert!(!parsed.panicked, "editor mode should recover {source_text:?}");
        assert_eq!(parsed.program.body.len(), 2);
        assert_eq!(parsed.diagnostics.len(), 1);
        assert_eq!(parsed.diagnostics[0].message, "',' expected.");
        assert_eq!(parsed.recoveries.len(), 1);
        assert_eq!(parsed.recoveries[0].kind, expected_kind);
    }
}

#[test]
fn editor_mode_recovers_empty_inner_lists_at_outer_closers() {
    for (source_text, message, code, expected_kind) in [
        (
            "const value = { nested: [}; const later = 1;",
            "Expression or comma expected.",
            "1137",
            "MissingClosingBracket",
        ),
        (
            "const value = [{]; const later = 2;",
            "Property assignment expected.",
            "1136",
            "MissingClosingBrace",
        ),
        (
            "function f(): void {} const value = [f(]; const later = 3;",
            "Argument expression expected.",
            "1135",
            "MissingClosingParenthesis",
        ),
    ] {
        let allocator = Allocator::default();
        let parsed = Parser::new(&allocator, source_text, SourceType::ts())
            .with_options(ParseOptions { mode: ParseMode::Editor, ..ParseOptions::default() })
            .parse();

        assert!(!parsed.panicked, "editor mode should recover {source_text:?}");
        assert_eq!(
            parsed.program.body.len(),
            if source_text.starts_with("function") { 3 } else { 2 }
        );
        assert_eq!(parsed.diagnostics.len(), 1);
        assert_eq!(parsed.diagnostics[0].message, message);
        assert_eq!(parsed.diagnostics[0].code.scope.as_deref(), Some("TS"));
        assert_eq!(parsed.diagnostics[0].code.number.as_deref(), Some(code));
        assert_eq!(parsed.recoveries.len(), 1);
        assert_eq!(parsed.recoveries[0].kind, expected_kind);
    }
}

#[test]
fn editor_mode_delimiter_recovery_is_deterministic_and_makes_progress() {
    let source_text =
        "const object = { first: 1 second: 2 }; const array = [1 2]; f(1 2); const later = 3;";
    let parse = || {
        let allocator = Allocator::default();
        let parsed = Parser::new(&allocator, source_text, SourceType::ts())
            .with_options(ParseOptions { mode: ParseMode::Editor, ..ParseOptions::default() })
            .parse();
        assert!(!parsed.panicked);
        assert_eq!(parsed.program.body.len(), 4);
        assert_eq!(parsed.diagnostics.len(), 3);
        assert_eq!(parsed.recoveries.len(), 3);
        assert!(parsed.recoveries.iter().all(|event| event.kind == "MissingComma"));

        (
            format!("{:#?}", parsed.program),
            parsed
                .diagnostics
                .iter()
                .map(|diagnostic| (diagnostic.message.to_string(), diagnostic.labels.clone()))
                .collect::<Vec<_>>(),
            parsed.recoveries.to_vec(),
        )
    };

    assert_eq!(parse(), parse());
}

#[test]
fn normal_mode_remains_fatal_for_missing_parameter_delimiters() {
    for source_text in
        ["function f(first: number second: string): void {}", "declare function f(value: number"]
    {
        let allocator = Allocator::default();
        let parsed = Parser::new(&allocator, source_text, SourceType::ts()).parse();

        assert!(parsed.panicked, "normal mode should abort for {source_text:?}");
        assert!(parsed.program.body.is_empty());
        assert!(parsed.recoveries.is_empty());
    }
}

#[test]
fn editor_mode_recovers_missing_parameter_commas() {
    let source_text = "function f(first: number second: string): void {} const later = 1;";
    let allocator = Allocator::default();
    let parsed = Parser::new(&allocator, source_text, SourceType::ts())
        .with_options(ParseOptions { mode: ParseMode::Editor, ..ParseOptions::default() })
        .parse();

    assert!(!parsed.panicked);
    assert_eq!(parsed.program.body.len(), 2);
    assert_eq!(parsed.diagnostics.len(), 1);
    assert_eq!(parsed.diagnostics[0].message, "',' expected.");
    assert_eq!(parsed.diagnostics[0].code.scope.as_deref(), Some("TS"));
    assert_eq!(parsed.diagnostics[0].code.number.as_deref(), Some("1005"));
    assert_eq!(parsed.recoveries.len(), 1);
    assert_eq!(parsed.recoveries[0].kind, "MissingComma");
    assert_eq!(parsed.recoveries[0].span, Span::empty(25));

    let oxc_ast::ast::Statement::FunctionDeclaration(function) = &parsed.program.body[0] else {
        panic!("expected a function declaration");
    };
    assert_eq!(function.params.items.len(), 2);
}

#[test]
fn normal_mode_remains_fatal_for_a_missing_parameter() {
    let allocator = Allocator::default();
    let parsed = Parser::new(
        &allocator,
        "function broken(, second: number): void {} const later = 2;",
        SourceType::ts(),
    )
    .parse();

    assert!(parsed.panicked);
    assert!(parsed.program.body.is_empty());
}

#[test]
fn editor_mode_recovers_a_missing_parameter_without_inventing_a_binding() {
    let source_text = "function broken(, second: number): void {} const later = 2;";
    let allocator = Allocator::default();
    let parsed = Parser::new(&allocator, source_text, SourceType::ts())
        .with_options(ParseOptions { mode: ParseMode::Editor, ..ParseOptions::default() })
        .parse();

    assert!(!parsed.panicked);
    assert_eq!(parsed.program.body.len(), 2);
    assert_eq!(parsed.diagnostics.len(), 1);
    assert_eq!(parsed.diagnostics[0].message, "Parameter declaration expected.");
    assert_eq!(parsed.diagnostics[0].code.scope.as_deref(), Some("TS"));
    assert_eq!(parsed.diagnostics[0].code.number.as_deref(), Some("1138"));
    assert_eq!(parsed.recoveries.len(), 1);
    assert_eq!(parsed.recoveries[0].kind, "MissingParameter");
    assert_eq!(parsed.recoveries[0].span, Span::empty(16));

    let oxc_ast::ast::Statement::FunctionDeclaration(function) = &parsed.program.body[0] else {
        panic!("expected a function declaration");
    };
    assert_eq!(function.params.items.len(), 1);
}

#[test]
fn editor_mode_recovers_missing_parameter_closer_at_eof() {
    let source_text = "declare function f(value: number";
    let allocator = Allocator::default();
    let parsed = Parser::new(&allocator, source_text, SourceType::ts())
        .with_options(ParseOptions { mode: ParseMode::Editor, ..ParseOptions::default() })
        .parse();

    assert!(!parsed.panicked);
    assert_eq!(parsed.program.body.len(), 1);
    assert_eq!(parsed.diagnostics.len(), 1);
    assert_eq!(parsed.diagnostics[0].message, "')' expected.");
    assert_eq!(parsed.diagnostics[0].code.scope.as_deref(), Some("TS"));
    assert_eq!(parsed.diagnostics[0].code.number.as_deref(), Some("1005"));
    assert_eq!(parsed.recoveries.len(), 1);
    assert_eq!(parsed.recoveries[0].kind, "MissingClosingParenthesis");
    assert_eq!(parsed.recoveries[0].span, Span::empty(32));
}

#[test]
fn normal_mode_remains_fatal_for_a_call_closed_by_a_following_declaration() {
    let allocator = Allocator::default();
    let parsed = Parser::new(
        &allocator,
        "function check(value: number): void {}\ncheck(\nconst later = 2;",
        SourceType::ts(),
    )
    .parse();

    assert!(parsed.panicked);
    assert!(parsed.program.body.is_empty());
}

#[test]
fn editor_mode_recovers_a_call_closed_by_a_following_declaration() {
    let source_text = "function check(value: number): void {}\ncheck(\nconst later = 2;";
    let allocator = Allocator::default();
    let parsed = Parser::new(&allocator, source_text, SourceType::ts())
        .with_options(ParseOptions { mode: ParseMode::Editor, ..ParseOptions::default() })
        .parse();

    assert!(!parsed.panicked);
    assert_eq!(parsed.program.body.len(), 3);
    assert_eq!(parsed.diagnostics.len(), 1);
    assert_eq!(parsed.diagnostics[0].message, "Argument expression expected.");
    assert_eq!(parsed.diagnostics[0].code.scope.as_deref(), Some("TS"));
    assert_eq!(parsed.diagnostics[0].code.number.as_deref(), Some("1135"));
    assert_eq!(parsed.recoveries.len(), 1);
    assert_eq!(parsed.recoveries[0].kind, "MissingClosingParenthesis");
    assert_eq!(parsed.recoveries[0].span, Span::empty(46));
}

#[test]
fn normal_mode_remains_fatal_for_a_missing_variable_declaration_name() {
    let allocator = Allocator::default();
    let parsed = Parser::new(&allocator, "const = 1;\nconst later = 2;", SourceType::ts()).parse();

    assert!(parsed.panicked);
    assert!(parsed.program.body.is_empty());
}

#[test]
fn editor_mode_recovers_a_missing_variable_declaration_name_without_a_binding() {
    let source_text = "const = 1;\nconst later = 2;";
    let allocator = Allocator::default();
    let parsed = Parser::new(&allocator, source_text, SourceType::ts())
        .with_options(ParseOptions { mode: ParseMode::Editor, ..ParseOptions::default() })
        .parse();

    assert!(!parsed.panicked);
    assert_eq!(parsed.program.body.len(), 3);
    assert_eq!(parsed.diagnostics.len(), 2);
    assert!(parsed.diagnostics.iter().all(|diagnostic| {
        diagnostic.message == "Variable declaration expected."
            && diagnostic.code.scope.as_deref() == Some("TS")
            && diagnostic.code.number.as_deref() == Some("1134")
    }));
    assert_eq!(parsed.recoveries.len(), 2);
    assert_eq!(parsed.recoveries[0].kind, "MissingDeclarationName");
    assert_eq!(parsed.recoveries[0].span, Span::new(6, 7));
    assert_eq!(parsed.recoveries[1].kind, "UnexpectedVariableInitializer");
    assert_eq!(parsed.recoveries[1].span, Span::new(8, 9));

    let oxc_ast::ast::Statement::VariableDeclaration(missing) = &parsed.program.body[0] else {
        panic!("expected a variable declaration");
    };
    assert!(missing.declarations.is_empty());
    assert!(matches!(parsed.program.body[1], oxc_ast::ast::Statement::ExpressionStatement(_)));
    let oxc_ast::ast::Statement::VariableDeclaration(later) = &parsed.program.body[2] else {
        panic!("expected the following variable declaration");
    };
    assert_eq!(later.declarations.len(), 1);
}

#[test]
fn editor_mode_deletion_mutation_matrix_is_deterministic_and_bounded() {
    let cases = [
        ("initializer", "const value = 1; const later = 2;", "1", 2),
        ("object comma", "const value = { first: 1, second: 2 }; const later = 3;", ",", 2),
        ("array comma", "const value = [1, 2]; const later = 3;", ",", 2),
        ("call comma", "check(1, 2); const later = 3;", ",", 2),
        (
            "interface separator",
            "interface Box { value: number; label: string } const later = 2;",
            ";",
            2,
        ),
        (
            "class separator",
            "class Box { value: number = 1; label: string = \"Box\"; } const later = 2;",
            ";",
            2,
        ),
        ("type argument closer", "const values: Array<number> = []; const later = 2;", ">", 2),
        (
            "parameter comma",
            "function f(value: number, suffix: string): void {} const later = 2;",
            ",",
            2,
        ),
        ("declaration name", "const value = 1; const later = 2;", "value", 3),
        ("call closer", "check()\nconst later = 2;", ")", 2),
        ("function closer", "function f(): void { const local = 1; }", "}", 1),
        ("class closer", "class Box { value: number = 1; }", "}", 1),
    ];

    for (name, complete, deleted, expected_statements) in cases {
        let normal_allocator = Allocator::default();
        let normal = Parser::new(&normal_allocator, complete, SourceType::ts()).parse();
        assert!(!normal.panicked, "valid seed aborted for {name}");
        assert!(normal.diagnostics.is_empty(), "invalid seed for {name}");

        let mutated = delete_first(complete, deleted);
        assert_ne!(mutated, complete, "mutation did not delete {deleted:?} for {name}");
        let parse = || {
            let allocator = Allocator::default();
            let parsed = Parser::new(&allocator, &mutated, SourceType::ts())
                .with_options(ParseOptions { mode: ParseMode::Editor, ..ParseOptions::default() })
                .parse();

            assert!(!parsed.panicked, "editor parse aborted for mutation {name}");
            assert_eq!(
                parsed.program.body.len(),
                expected_statements,
                "surviving statements differ for mutation {name}"
            );
            assert!(!parsed.diagnostics.is_empty(), "mutation {name} produced no diagnostic");

            let source_len = u32::try_from(mutated.len()).expect("test source fits in u32");
            let mut bounded = BoundedSpans { source_len, node_count: 0, recovery_count: 0 };
            bounded.visit_program(&parsed.program);
            assert!(bounded.node_count > 0);
            assert!(
                bounded.recovery_count + parsed.recoveries.len() > 0,
                "mutation {name} produced no recovery site"
            );
            for diagnostic in &parsed.diagnostics {
                for label in &diagnostic.labels {
                    assert!(label.offset() + label.len() <= source_len);
                }
            }
            for recovery in &parsed.recoveries {
                assert!(recovery.span.start <= recovery.span.end);
                assert!(recovery.span.end <= source_len);
            }

            (
                format!("{:#?}", parsed.program),
                parsed
                    .diagnostics
                    .iter()
                    .map(|diagnostic| (diagnostic.message.to_string(), diagnostic.labels.clone()))
                    .collect::<Vec<_>>(),
                parsed.recoveries.clone(),
            )
        };

        assert_eq!(parse(), parse(), "mutation {name} was not deterministic");
    }
}

#[test]
fn editor_mode_recovers_parameter_closers_owned_by_an_outer_interface() {
    let source_text = "interface Handler { run(value: number } const later = 1;";
    let allocator = Allocator::default();
    let parsed = Parser::new(&allocator, source_text, SourceType::ts())
        .with_options(ParseOptions { mode: ParseMode::Editor, ..ParseOptions::default() })
        .parse();

    assert!(!parsed.panicked);
    assert_eq!(parsed.program.body.len(), 2);
    assert_eq!(parsed.diagnostics.len(), 1);
    assert_eq!(parsed.diagnostics[0].message, "',' expected.");
    assert_eq!(parsed.diagnostics[0].code.scope.as_deref(), Some("TS"));
    assert_eq!(parsed.diagnostics[0].code.number.as_deref(), Some("1005"));
    assert_eq!(parsed.recoveries.len(), 1);
    assert_eq!(parsed.recoveries[0].kind, "MissingClosingParenthesis");
    assert_eq!(parsed.recoveries[0].span, Span::empty(38));
}

#[test]
fn editor_mode_does_not_change_valid_parameter_input() {
    let source_text = "function f(first: number, second?: string, ...rest: boolean[]): void {}";
    let normal_allocator = Allocator::default();
    let normal = Parser::new(&normal_allocator, source_text, SourceType::ts()).parse();
    let editor_allocator = Allocator::default();
    let editor = Parser::new(&editor_allocator, source_text, SourceType::ts())
        .with_options(ParseOptions { mode: ParseMode::Editor, ..ParseOptions::default() })
        .parse();

    assert!(normal.diagnostics.is_empty());
    assert!(editor.diagnostics.is_empty());
    assert_eq!(format!("{:#?}", normal.program), format!("{:#?}", editor.program));
}

#[test]
fn normal_mode_remains_fatal_for_a_missing_function_body_closer() {
    let allocator = Allocator::default();
    let parsed =
        Parser::new(&allocator, "function f(): number { return 1;", SourceType::ts()).parse();

    assert!(parsed.panicked);
    assert!(parsed.program.body.is_empty());
    assert!(parsed.recoveries.is_empty());
}

#[test]
fn editor_mode_recovers_a_missing_function_body_closer_at_eof() {
    let source_text = "function f(): number { return 1;";
    let allocator = Allocator::default();
    let parsed = Parser::new(&allocator, source_text, SourceType::ts())
        .with_options(ParseOptions { mode: ParseMode::Editor, ..ParseOptions::default() })
        .parse();

    assert!(!parsed.panicked);
    assert_eq!(parsed.program.body.len(), 1);
    assert_eq!(parsed.diagnostics.len(), 1);
    assert_eq!(parsed.diagnostics[0].message, "'}' expected.");
    assert_eq!(parsed.diagnostics[0].code.scope.as_deref(), Some("TS"));
    assert_eq!(parsed.diagnostics[0].code.number.as_deref(), Some("1005"));
    assert_eq!(parsed.recoveries.len(), 1);
    assert_eq!(parsed.recoveries[0].kind, "MissingClosingBrace");
    assert_eq!(parsed.recoveries[0].span, Span::empty(32));
}

#[test]
fn normal_mode_remains_fatal_for_a_missing_return_expression_operand() {
    let allocator = Allocator::default();
    let parsed = Parser::new(
        &allocator,
        "function f(): number { return 1 + } const later = 2;",
        SourceType::ts(),
    )
    .parse();

    assert!(parsed.panicked);
    assert!(parsed.program.body.is_empty());
}

#[test]
fn editor_mode_recovers_a_missing_return_expression_operand() {
    let source_text = "function f(): number { return 1 + } const later = 2;";
    let allocator = Allocator::default();
    let parsed = Parser::new(&allocator, source_text, SourceType::ts())
        .with_options(ParseOptions { mode: ParseMode::Editor, ..ParseOptions::default() })
        .parse();

    assert!(!parsed.panicked);
    assert_eq!(parsed.program.body.len(), 2);
    assert_eq!(parsed.diagnostics.len(), 1);
    assert_eq!(parsed.diagnostics[0].message, "Expression expected.");
    assert_eq!(parsed.diagnostics[0].code.scope.as_deref(), Some("TS"));
    assert_eq!(parsed.diagnostics[0].code.number.as_deref(), Some("1109"));

    let mut missing = MissingExpressions::default();
    missing.visit_program(&parsed.program);
    assert_eq!(missing.spans, [Span::empty(34)]);
    assert_eq!(missing.ast_kind_count, 1);
}

#[test]
fn normal_mode_remains_fatal_for_a_missing_interface_closer() {
    let allocator = Allocator::default();
    let parsed =
        Parser::new(&allocator, "interface Box { value: number;", SourceType::ts()).parse();

    assert!(parsed.panicked);
    assert!(parsed.program.body.is_empty());
    assert!(parsed.recoveries.is_empty());
}

#[test]
fn editor_mode_recovers_a_missing_interface_closer_at_eof() {
    let source_text = "interface Box { value: number;";
    let offset = u32::try_from(source_text.len()).expect("source length fits in u32");
    let allocator = Allocator::default();
    let parsed = Parser::new(&allocator, source_text, SourceType::ts())
        .with_options(ParseOptions { mode: ParseMode::Editor, ..ParseOptions::default() })
        .parse();

    assert!(!parsed.panicked);
    assert_eq!(parsed.program.body.len(), 1);
    assert_eq!(parsed.diagnostics.len(), 1);
    assert_eq!(parsed.diagnostics[0].message, "'}' expected.");
    assert_eq!(parsed.diagnostics[0].code.scope.as_deref(), Some("TS"));
    assert_eq!(parsed.diagnostics[0].code.number.as_deref(), Some("1005"));
    assert_eq!(parsed.recoveries.len(), 1);
    assert_eq!(parsed.recoveries[0].kind, "MissingClosingBrace");
    assert_eq!(parsed.recoveries[0].span, Span::empty(offset));
}

#[test]
fn editor_mode_does_not_change_valid_interface_input() {
    let source_text = "interface Box { value: number; optional?: string }";
    let normal_allocator = Allocator::default();
    let normal = Parser::new(&normal_allocator, source_text, SourceType::ts()).parse();
    let editor_allocator = Allocator::default();
    let editor = Parser::new(&editor_allocator, source_text, SourceType::ts())
        .with_options(ParseOptions { mode: ParseMode::Editor, ..ParseOptions::default() })
        .parse();

    assert!(normal.diagnostics.is_empty());
    assert!(editor.diagnostics.is_empty());
    assert_eq!(format!("{:#?}", normal.program), format!("{:#?}", editor.program));
}

#[test]
fn editor_mode_recovers_a_missing_interface_member_separator() {
    let source_text = "interface Box { first: number second: string } const later = 2;";
    let allocator = Allocator::default();
    let parsed = Parser::new(&allocator, source_text, SourceType::ts())
        .with_options(ParseOptions { mode: ParseMode::Editor, ..ParseOptions::default() })
        .parse();

    assert!(!parsed.panicked);
    assert_eq!(parsed.program.body.len(), 2);
    assert_eq!(parsed.diagnostics.len(), 1);
    assert_eq!(parsed.diagnostics[0].message, "';' expected.");
    assert_eq!(parsed.diagnostics[0].code.scope.as_deref(), Some("TS"));
    assert_eq!(parsed.diagnostics[0].code.number.as_deref(), Some("1005"));
    assert_eq!(parsed.recoveries.len(), 1);
    assert_eq!(parsed.recoveries[0].kind, "MissingSemicolon");
    assert_eq!(parsed.recoveries[0].span, Span::empty(30));

    let oxc_ast::ast::Statement::TSInterfaceDeclaration(interface) = &parsed.program.body[0] else {
        panic!("expected an interface declaration");
    };
    assert_eq!(interface.body.body.len(), 2);
}

#[test]
fn normal_mode_remains_fatal_for_a_missing_class_member_separator() {
    let allocator = Allocator::default();
    let parsed = Parser::new(
        &allocator,
        "class Box { first: number = 1 second: string = \"ok\"; }",
        SourceType::ts(),
    )
    .parse();

    assert!(parsed.panicked);
    assert!(parsed.program.body.is_empty());
    assert!(parsed.recoveries.is_empty());
}

#[test]
fn editor_mode_recovers_a_missing_class_member_separator() {
    let source_text = "class Box { first: number = 1 second: string = \"ok\"; } const later = 2;";
    let offset = u32::try_from(source_text.find("second").expect("fixture contains second"))
        .expect("source offset fits in u32");
    let allocator = Allocator::default();
    let parsed = Parser::new(&allocator, source_text, SourceType::ts())
        .with_options(ParseOptions { mode: ParseMode::Editor, ..ParseOptions::default() })
        .parse();

    assert!(!parsed.panicked);
    assert_eq!(parsed.program.body.len(), 2);
    assert_eq!(parsed.diagnostics.len(), 1);
    assert_eq!(parsed.diagnostics[0].message, "';' expected.");
    assert_eq!(parsed.diagnostics[0].code.scope.as_deref(), Some("TS"));
    assert_eq!(parsed.diagnostics[0].code.number.as_deref(), Some("1005"));
    assert_eq!(parsed.recoveries.len(), 1);
    assert_eq!(parsed.recoveries[0].kind, "MissingSemicolon");
    assert_eq!(parsed.recoveries[0].span, Span::empty(offset));

    let oxc_ast::ast::Statement::ClassDeclaration(class) = &parsed.program.body[0] else {
        panic!("expected a class declaration");
    };
    assert_eq!(class.body.body.len(), 2);
}

#[test]
fn editor_mode_recovers_a_missing_class_closer_at_eof() {
    let source_text = "class Box { value: number = 1;";
    let offset = u32::try_from(source_text.len()).expect("source length fits in u32");
    let allocator = Allocator::default();
    let parsed = Parser::new(&allocator, source_text, SourceType::ts())
        .with_options(ParseOptions { mode: ParseMode::Editor, ..ParseOptions::default() })
        .parse();

    assert!(!parsed.panicked);
    assert_eq!(parsed.program.body.len(), 1);
    assert_eq!(parsed.diagnostics.len(), 1);
    assert_eq!(parsed.diagnostics[0].message, "'}' expected.");
    assert_eq!(parsed.recoveries.len(), 1);
    assert_eq!(parsed.recoveries[0].kind, "MissingClosingBrace");
    assert_eq!(parsed.recoveries[0].span, Span::empty(offset));
}

#[test]
fn editor_mode_does_not_change_valid_class_input() {
    let source_text = "class Box { value: number = 1; read(): number { return this.value; } }";
    let normal_allocator = Allocator::default();
    let normal = Parser::new(&normal_allocator, source_text, SourceType::ts()).parse();
    let editor_allocator = Allocator::default();
    let editor = Parser::new(&editor_allocator, source_text, SourceType::ts())
        .with_options(ParseOptions { mode: ParseMode::Editor, ..ParseOptions::default() })
        .parse();

    assert!(normal.diagnostics.is_empty());
    assert!(editor.diagnostics.is_empty());
    assert_eq!(format!("{:#?}", normal.program), format!("{:#?}", editor.program));
}

#[test]
fn normal_mode_remains_fatal_for_a_missing_member_name() {
    let allocator = Allocator::default();
    let parsed = Parser::new(
        &allocator,
        "const box = { value: 1 }; box.; const later = 2;",
        SourceType::ts(),
    )
    .parse();

    assert!(parsed.panicked);
    assert!(parsed.program.body.is_empty());
}

#[test]
fn editor_mode_recovers_a_missing_member_name_at_a_statement_boundary() {
    let source_text = "const box = { value: 1 }; box.; const later = 2;";
    let missing_offset =
        u32::try_from(source_text.find("; const later").expect("missing member boundary"))
            .expect("source offset fits in u32");
    let allocator = Allocator::default();
    let parsed = Parser::new(&allocator, source_text, SourceType::ts())
        .with_options(ParseOptions { mode: ParseMode::Editor, ..ParseOptions::default() })
        .parse();

    assert!(!parsed.panicked);
    assert_eq!(parsed.program.body.len(), 3);
    assert_eq!(parsed.diagnostics.len(), 1);
    assert_eq!(parsed.diagnostics[0].message, "Identifier expected.");
    assert_eq!(parsed.diagnostics[0].code.scope.as_deref(), Some("TS"));
    assert_eq!(parsed.diagnostics[0].code.number.as_deref(), Some("1003"));

    let mut missing = MissingMemberExpressions::default();
    missing.visit_program(&parsed.program);
    assert_eq!(missing.spans, [Span::new(26, missing_offset)]);
    assert_eq!(missing.missing_property_spans, [Span::empty(missing_offset)]);
    assert_eq!(missing.ast_kind_count, 1);
}

#[test]
fn editor_mode_recovers_a_missing_optional_member_name() {
    let source_text = "declare const box: { value: number }; box?.; const later = 2;";
    let missing_offset =
        u32::try_from(source_text.find("; const later").expect("missing member boundary"))
            .expect("source offset fits in u32");
    let allocator = Allocator::default();
    let parsed = Parser::new(&allocator, source_text, SourceType::ts())
        .with_options(ParseOptions { mode: ParseMode::Editor, ..ParseOptions::default() })
        .parse();

    assert!(!parsed.panicked);
    assert_eq!(parsed.program.body.len(), 3);
    assert_eq!(parsed.diagnostics.len(), 1);
    assert_eq!(parsed.diagnostics[0].message, "Identifier expected.");
    assert_eq!(parsed.diagnostics[0].code.scope.as_deref(), Some("TS"));
    assert_eq!(parsed.diagnostics[0].code.number.as_deref(), Some("1003"));

    let mut missing = MissingMemberExpressions::default();
    missing.visit_program(&parsed.program);
    assert_eq!(missing.missing_property_spans, [Span::empty(missing_offset)]);
    assert_eq!(missing.ast_kind_count, 1);
}

#[test]
fn editor_mode_recovers_at_supported_assignment_rhs_boundaries() {
    for source_text in [
        "let value = 1; value =",
        "let value = 1; value = ; const intact = 2;",
        "function f() { let value = 1; value = }",
        "let value = 1; value += ; const intact = 2;",
    ] {
        let offset = u32::try_from(
            source_text
                .rfind("; const intact")
                .or_else(|| source_text.rfind('}'))
                .unwrap_or(source_text.len()),
        )
        .expect("test source offset fits in u32");
        assert_editor_missing_expression(source_text, offset);
    }
}

#[test]
fn editor_mode_puts_the_missing_expression_on_the_assignment_rhs() {
    let allocator = Allocator::default();
    let parsed =
        Parser::new(&allocator, "let target = 1; target = ; const intact = 2;", SourceType::ts())
            .with_options(ParseOptions { mode: ParseMode::Editor, ..ParseOptions::default() })
            .parse();

    assert!(!parsed.panicked);
    assert_eq!(parsed.program.body.len(), 3);
    let oxc_ast::ast::Statement::ExpressionStatement(statement) = &parsed.program.body[1] else {
        panic!("expected an expression statement");
    };
    let Expression::AssignmentExpression(assignment) = &statement.expression else {
        panic!("expected an assignment expression");
    };
    assert!(matches!(assignment.right, Expression::MissingExpression(_)));
}

#[test]
fn editor_mode_assignment_recovery_is_deterministic_and_makes_progress() {
    let source_text = "let target = 1; target = ; target += ; const intact = 2;";
    let parse = || {
        let allocator = Allocator::default();
        let parsed = Parser::new(&allocator, source_text, SourceType::ts())
            .with_options(ParseOptions { mode: ParseMode::Editor, ..ParseOptions::default() })
            .parse();
        assert!(!parsed.panicked);
        assert_eq!(parsed.program.body.len(), 4);
        assert_eq!(parsed.diagnostics.len(), 2);

        let mut missing = MissingExpressions::default();
        missing.visit_program(&parsed.program);
        assert_eq!(missing.spans, [Span::empty(25), Span::empty(37)]);
        assert_eq!(missing.ast_kind_count, 2);

        (
            format!("{:#?}", parsed.program),
            parsed
                .diagnostics
                .iter()
                .map(|diagnostic| (diagnostic.message.to_string(), diagnostic.labels.clone()))
                .collect::<Vec<_>>(),
        )
    };

    assert_eq!(parse(), parse());
}

#[test]
fn editor_mode_does_not_change_valid_assignment_input() {
    let source_text = "let target = 1; target = 2; target += 3;";
    let normal_allocator = Allocator::default();
    let normal = Parser::new(&normal_allocator, source_text, SourceType::ts()).parse();
    let editor_allocator = Allocator::default();
    let editor = Parser::new(&editor_allocator, source_text, SourceType::ts())
        .with_options(ParseOptions { mode: ParseMode::Editor, ..ParseOptions::default() })
        .parse();

    assert!(normal.diagnostics.is_empty());
    assert!(editor.diagnostics.is_empty());
    assert_eq!(format!("{:#?}", normal.program), format!("{:#?}", editor.program));
}

#[test]
fn editor_mode_does_not_change_valid_input() {
    let source_text = "const value: number = 1;";
    let normal_allocator = Allocator::default();
    let normal = Parser::new(&normal_allocator, source_text, SourceType::ts()).parse();
    let editor_allocator = Allocator::default();
    let editor = Parser::new(&editor_allocator, source_text, SourceType::ts())
        .with_options(ParseOptions { mode: ParseMode::Editor, ..ParseOptions::default() })
        .parse();

    assert!(normal.diagnostics.is_empty());
    assert!(editor.diagnostics.is_empty());
    assert_eq!(format!("{:#?}", normal.program), format!("{:#?}", editor.program));
}

#[test]
fn expression_layout_is_unchanged() {
    assert_eq!(size_of::<Expression<'_>>(), 16);
}

#[test]
fn type_layout_is_unchanged() {
    assert_eq!(size_of::<TSType<'_>>(), 16);
}

#[test]
fn normal_mode_remains_fatal_for_missing_types() {
    for source_text in [
        "const broken: = 1; const intact: number = 2;",
        "type Broken = ; const intact: number = 2;",
        "type Broken = string | ; const intact: number = 2;",
        "type Shape = { missing: ; wrong: number }; const intact: number = 2;",
        "declare function broken(): ; const intact: number = 2;",
        "interface Box { value: } const intact: number = 2;",
    ] {
        let allocator = Allocator::default();
        let parsed = Parser::new(&allocator, source_text, SourceType::ts()).parse();
        assert!(parsed.panicked, "normal mode should abort for {source_text:?}");
        assert!(parsed.program.body.is_empty());
    }
}

#[test]
fn editor_mode_recovers_missing_types_at_owned_boundaries() {
    for (source_text, offset) in [
        ("const broken: = 1; const intact: number = 2;", 14),
        ("type Broken = ; const intact: number = 2;", 14),
        ("type Broken = string | ; const intact: number = 2;", 23),
        ("type Shape = { missing: ; wrong: number }; const intact: number = 2;", 24),
        ("declare function broken(): ; const intact: number = 2;", 27),
        ("interface Box { value: } const intact: number = 2;", 23),
    ] {
        let allocator = Allocator::default();
        let parsed = Parser::new(&allocator, source_text, SourceType::ts())
            .with_options(ParseOptions { mode: ParseMode::Editor, ..ParseOptions::default() })
            .parse();

        assert!(!parsed.panicked, "editor mode should recover {source_text:?}");
        assert_eq!(parsed.program.body.len(), 2);
        assert_eq!(parsed.diagnostics.len(), 1);
        assert_eq!(parsed.diagnostics[0].message, "Type expected.");
        assert_eq!(parsed.diagnostics[0].code.scope.as_deref(), Some("TS"));
        assert_eq!(parsed.diagnostics[0].code.number.as_deref(), Some("1110"));

        let mut missing = MissingTypes::default();
        missing.visit_program(&parsed.program);
        assert_eq!(missing.spans, [Span::empty(offset)]);
        assert_eq!(missing.ast_kind_count, 1);
    }
}

#[test]
fn editor_mode_missing_union_constituents_are_deterministic_and_make_progress() {
    let source_text = "type Broken = string | | number | ; const intact: number = 2;";
    let parse = || {
        let allocator = Allocator::default();
        let parsed = Parser::new(&allocator, source_text, SourceType::ts())
            .with_options(ParseOptions { mode: ParseMode::Editor, ..ParseOptions::default() })
            .parse();
        assert!(!parsed.panicked);
        assert_eq!(parsed.program.body.len(), 2);
        assert_eq!(parsed.diagnostics.len(), 2);

        let mut missing = MissingTypes::default();
        missing.visit_program(&parsed.program);
        assert_eq!(missing.spans, [Span::empty(23), Span::empty(34)]);
        assert_eq!(missing.ast_kind_count, 2);

        (
            format!("{:#?}", parsed.program),
            parsed
                .diagnostics
                .iter()
                .map(|diagnostic| (diagnostic.message.to_string(), diagnostic.labels.clone()))
                .collect::<Vec<_>>(),
        )
    };

    assert_eq!(parse(), parse());
}

#[test]
fn editor_mode_does_not_change_valid_type_input() {
    let source_text = "type Value = string | number; const value: Value = 1;";
    let normal_allocator = Allocator::default();
    let editor_allocator = Allocator::default();
    let normal = Parser::new(&normal_allocator, source_text, SourceType::ts()).parse();
    let editor = Parser::new(&editor_allocator, source_text, SourceType::ts())
        .with_options(ParseOptions { mode: ParseMode::Editor, ..ParseOptions::default() })
        .parse();

    assert!(normal.diagnostics.is_empty());
    assert!(editor.diagnostics.is_empty());
    assert_eq!(format!("{:#?}", normal.program), format!("{:#?}", editor.program));
}

#[test]
fn normal_mode_remains_fatal_for_missing_type_closers() {
    for source_text in [
        "type Values = number[; const intact: number = 2;",
        "type Value = (number; const intact: number = 2;",
        "const value: Array<number = []; const intact: number = 2;",
    ] {
        let allocator = Allocator::default();
        let parsed = Parser::new(&allocator, source_text, SourceType::ts()).parse();
        assert!(parsed.panicked, "normal mode should abort for {source_text:?}");
        assert!(parsed.program.body.is_empty());
    }
}

#[test]
fn editor_mode_recovers_missing_type_closers_at_owned_boundaries() {
    for (source_text, expected_kind, offset, message) in [
        (
            "type Values = number[; const intact: number = 2;",
            "MissingClosingBracket",
            21,
            "']' expected.",
        ),
        (
            "type Value = (number; const intact: number = 2;",
            "MissingClosingParenthesis",
            20,
            "')' expected.",
        ),
        (
            "const value: Array<number = []; const intact: number = 2;",
            "MissingClosingAngleBracket",
            26,
            "'>' expected.",
        ),
    ] {
        let allocator = Allocator::default();
        let parsed = Parser::new(&allocator, source_text, SourceType::ts())
            .with_options(ParseOptions { mode: ParseMode::Editor, ..ParseOptions::default() })
            .parse();

        assert!(!parsed.panicked, "editor mode should recover {source_text:?}");
        assert_eq!(parsed.program.body.len(), 2);
        assert_eq!(parsed.diagnostics.len(), 1);
        assert_eq!(parsed.diagnostics[0].message, message);
        assert_eq!(parsed.recoveries.len(), 1);
        assert_eq!(parsed.recoveries[0].kind, expected_kind);
        assert_eq!(parsed.recoveries[0].span, Span::empty(offset));

        let mut missing = MissingTypes::default();
        missing.visit_program(&parsed.program);
        assert!(missing.spans.is_empty());
    }
}

#[test]
fn normal_mode_remains_fatal_for_source_backed_malformed_expressions() {
    for source_text in [
        "const broken: number = :; const intact: number = 2;",
        "let target = 1; target = ...; const intact: number = 2;",
    ] {
        let allocator = Allocator::default();
        let parsed = Parser::new(&allocator, source_text, SourceType::ts()).parse();
        assert!(parsed.panicked, "normal mode should abort for {source_text:?}");
        assert!(parsed.program.body.is_empty());
    }
}

#[test]
fn editor_mode_preserves_source_backed_malformed_expressions() {
    for (source_text, malformed_source, start, statement_count) in [
        ("const broken: number = :; const intact: number = 2;", ":", 23, 2),
        ("let target = 1; target = ...; const intact: number = 2;", "...", 25, 3),
    ] {
        let allocator = Allocator::default();
        let parsed = Parser::new(&allocator, source_text, SourceType::ts())
            .with_options(ParseOptions { mode: ParseMode::Editor, ..ParseOptions::default() })
            .parse();

        assert!(!parsed.panicked, "editor mode should recover {source_text:?}");
        assert_eq!(parsed.program.body.len(), statement_count);
        assert_eq!(parsed.diagnostics.len(), 1);
        assert_eq!(parsed.diagnostics[0].message, "Expression expected.");
        assert_eq!(parsed.diagnostics[0].code.scope.as_deref(), Some("TS"));
        assert_eq!(parsed.diagnostics[0].code.number.as_deref(), Some("1109"));

        let end = start + u32::try_from(malformed_source.len()).expect("token length fits in u32");
        let mut malformed = MalformedExpressions::default();
        malformed.visit_program(&parsed.program);
        assert_eq!(malformed.spans, [Span::new(start, end)]);
        assert_eq!(malformed.ast_kind_count, 1);
    }
}

#[test]
fn editor_mode_malformed_expression_recovery_is_deterministic_and_makes_progress() {
    let source_text = concat!(
        "const first: number = :; ",
        "let target = 1; target = ...; ",
        "const intact: number = 2;",
    );
    let parse = || {
        let allocator = Allocator::default();
        let parsed = Parser::new(&allocator, source_text, SourceType::ts())
            .with_options(ParseOptions { mode: ParseMode::Editor, ..ParseOptions::default() })
            .parse();
        assert!(!parsed.panicked);
        assert_eq!(parsed.program.body.len(), 4);
        assert_eq!(parsed.diagnostics.len(), 2);

        let mut malformed = MalformedExpressions::default();
        malformed.visit_program(&parsed.program);
        assert_eq!(malformed.spans, [Span::new(22, 23), Span::new(50, 53)]);
        assert_eq!(malformed.ast_kind_count, 2);

        (
            format!("{:#?}", parsed.program),
            parsed
                .diagnostics
                .iter()
                .map(|diagnostic| (diagnostic.message.to_string(), diagnostic.labels.clone()))
                .collect::<Vec<_>>(),
        )
    };

    assert_eq!(parse(), parse());
}
