use std::mem::size_of;

use oxc_allocator::Allocator;
use oxc_ast::{AstKind, ast::Expression};
use oxc_ast_visit::{Visit, walk};
use oxc_parser::{ParseMode, ParseOptions, Parser};
use oxc_span::{SourceType, Span};

#[derive(Default)]
struct MissingExpressions {
    spans: Vec<Span>,
    ast_kind_count: usize,
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
    let allocator = Allocator::default();
    let parsed = Parser::new(&allocator, "let value =", SourceType::ts()).parse();

    assert!(parsed.panicked);
    assert!(parsed.program.body.is_empty());
    assert!(!parsed.diagnostics.is_empty());
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
