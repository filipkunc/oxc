use serde::Serialize;

use oxc::{
    allocator::Allocator,
    ast::{
        AstKind,
        ast::{BindingPattern, Program, Statement},
    },
    ast_visit::Visit,
    diagnostics::OxcDiagnostic,
    parser::{ParseMode, ParseOptions, Parser, ParserReturn},
    semantic::SemanticBuilder,
    span::{GetSpan, SourceType},
};

use crate::OxcRecoveryInspectionOptions;

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RecoveryInspection {
    mode: &'static str,
    status: &'static str,
    statement_count: usize,
    diagnostic_count: usize,
    recovery_site_count: usize,
    diagnostics: Vec<InspectionDiagnostic>,
    tree: RecoveryNode,
    recovery_sites: Vec<RecoverySite>,
    declaration_names: Vec<String>,
    semantic: Option<SemanticSummary>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct InspectionDiagnostic {
    message: String,
    labels: Vec<InspectionRange>,
}

impl From<&OxcDiagnostic> for InspectionDiagnostic {
    fn from(diagnostic: &OxcDiagnostic) -> Self {
        Self {
            message: diagnostic.message.to_string(),
            labels: diagnostic
                .labels
                .iter()
                .map(|label| InspectionRange {
                    start: label.offset(),
                    end: label.offset() + label.len(),
                })
                .collect(),
        }
    }
}

#[derive(Debug, Clone, Copy, Serialize)]
struct InspectionRange {
    start: u32,
    end: u32,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct RecoveryNode {
    kind: &'static str,
    start: u32,
    end: u32,
    recovered: bool,
    label: Option<String>,
    children: Vec<RecoveryNode>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct RecoverySite {
    kind: &'static str,
    start: u32,
    end: u32,
    diagnostic_index: Option<usize>,
    parent_path: Vec<&'static str>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct SemanticSummary {
    binding_names: Vec<String>,
    reference_count: usize,
    diagnostics: Vec<InspectionDiagnostic>,
}

#[derive(Default)]
struct RecoveryTreeBuilder {
    stack: Vec<RecoveryNode>,
    root: Option<RecoveryNode>,
    recovery_sites: Vec<RecoverySite>,
}

impl<'a> Visit<'a> for RecoveryTreeBuilder {
    fn enter_node(&mut self, kind: AstKind<'a>) {
        let span = kind.span();
        let recovery_span = match kind {
            AstKind::MissingExpression(_)
            | AstKind::MalformedExpression(_)
            | AstKind::MissingType(_) => Some(span),
            AstKind::MissingMemberExpression(expression) => Some(expression.missing_property_span),
            _ => None,
        };
        let recovered = recovery_span.is_some();
        if let Some(recovery_span) = recovery_span {
            self.recovery_sites.push(RecoverySite {
                kind: kind.kind_name(),
                start: recovery_span.start,
                end: recovery_span.end,
                diagnostic_index: None,
                parent_path: self.stack.iter().map(|node| node.kind).collect(),
            });
        }
        let label = match kind {
            AstKind::BindingIdentifier(identifier) => Some(identifier.name.to_string()),
            AstKind::IdentifierReference(identifier) => Some(identifier.name.to_string()),
            _ => None,
        };
        self.stack.push(RecoveryNode {
            kind: kind.kind_name(),
            start: span.start,
            end: span.end,
            recovered,
            label,
            children: Vec::new(),
        });
    }

    fn leave_node(&mut self, kind: AstKind<'a>) {
        let node = self.stack.pop().expect("visitor leave has a matching enter");
        debug_assert_eq!(node.kind, kind.kind_name());
        if let Some(parent) = self.stack.last_mut() {
            parent.children.push(node);
        } else {
            debug_assert!(self.root.is_none());
            self.root = Some(node);
        }
    }
}

pub fn inspect_recovery(
    source_text: &str,
    options: &OxcRecoveryInspectionOptions,
) -> Result<RecoveryInspection, String> {
    let source_type = SourceType::from_path(format!("test.{}", options.extension))
        .map_err(|error| error.to_string())?;
    let mode = match options.mode.as_str() {
        "normal" => ParseMode::Normal,
        "editor" => ParseMode::Editor,
        mode => return Err(format!("unknown recovery inspection mode '{mode}'")),
    };

    let allocator = Allocator::default();
    let ParserReturn { program, diagnostics, recoveries, panicked, .. } =
        Parser::new(&allocator, source_text, source_type)
            .with_options(ParseOptions { mode, ..ParseOptions::default() })
            .parse();
    let diagnostic_summaries =
        diagnostics.iter().map(InspectionDiagnostic::from).collect::<Vec<_>>();

    let mut tree_builder = RecoveryTreeBuilder::default();
    tree_builder.visit_program(&program);
    tree_builder.recovery_sites.extend(recoveries.iter().map(|recovery| RecoverySite {
        kind: recovery.kind,
        start: recovery.span.start,
        end: recovery.span.end,
        diagnostic_index: None,
        parent_path: vec!["Program"],
    }));
    for site in &mut tree_builder.recovery_sites {
        site.diagnostic_index = diagnostic_summaries.iter().position(|diagnostic| {
            diagnostic.labels.iter().any(|label| {
                label.start == site.start && (label.end == site.end || site.start == site.end)
            })
        });
    }

    let semantic = if options.semantic && !panicked {
        let built = SemanticBuilder::new_compiler().with_build_nodes(true).build(&program);
        let mut binding_names = built
            .semantic
            .scoping()
            .get_bindings(built.semantic.scoping().root_scope_id())
            .keys()
            .map(ToString::to_string)
            .collect::<Vec<_>>();
        binding_names.sort_unstable();
        Some(SemanticSummary {
            binding_names,
            reference_count: built.semantic.scoping().references_len(),
            diagnostics: built.diagnostics.iter().map(InspectionDiagnostic::from).collect(),
        })
    } else {
        None
    };

    let declaration_names = top_level_declaration_names(&program);
    let status = if panicked {
        "aborted"
    } else if !diagnostics.is_empty() {
        "recovered"
    } else {
        "clean"
    };
    let recovery_site_count = tree_builder.recovery_sites.len();

    Ok(RecoveryInspection {
        mode: match mode {
            ParseMode::Normal => "normal",
            ParseMode::Editor => "editor",
        },
        status,
        statement_count: program.body.len(),
        diagnostic_count: diagnostic_summaries.len(),
        recovery_site_count,
        diagnostics: diagnostic_summaries,
        tree: tree_builder.root.expect("program visitor produces one root"),
        recovery_sites: tree_builder.recovery_sites,
        declaration_names,
        semantic,
    })
}

fn top_level_declaration_names(program: &Program<'_>) -> Vec<String> {
    let mut names = Vec::new();
    for statement in &program.body {
        match statement {
            Statement::VariableDeclaration(declaration) => {
                names.extend(declaration.declarations.iter().filter_map(|declarator| {
                    let BindingPattern::BindingIdentifier(identifier) = &declarator.id else {
                        return None;
                    };
                    Some(identifier.name.to_string())
                }));
            }
            Statement::FunctionDeclaration(function) => {
                names.extend(function.id.iter().map(|identifier| identifier.name.to_string()));
            }
            Statement::ClassDeclaration(class) => {
                names.extend(class.id.iter().map(|identifier| identifier.name.to_string()));
            }
            Statement::TSInterfaceDeclaration(interface) => {
                names.push(interface.id.name.to_string());
            }
            Statement::TSTypeAliasDeclaration(alias) => names.push(alias.id.name.to_string()),
            _ => {}
        }
    }
    names
}

#[cfg(test)]
mod tests {
    use super::*;

    fn options(mode: &str) -> OxcRecoveryInspectionOptions {
        OxcRecoveryInspectionOptions {
            extension: "ts".to_string(),
            mode: mode.to_string(),
            semantic: true,
        }
    }

    #[test]
    fn editor_inspection_reports_recovery_tree_and_surviving_bindings() {
        let inspection =
            inspect_recovery("const broken = ; const intact: number = 1;", &options("editor"))
                .unwrap();

        assert_eq!(inspection.status, "recovered");
        assert_eq!(inspection.statement_count, 2);
        assert_eq!(inspection.diagnostic_count, 1);
        assert_eq!(inspection.recovery_site_count, 1);
        assert_eq!(inspection.declaration_names, ["broken", "intact"]);
        assert_eq!(inspection.recovery_sites[0].kind, "MissingExpression");
        assert_eq!(inspection.recovery_sites[0].start, 15);
        assert_eq!(inspection.recovery_sites[0].end, 15);
        assert_eq!(inspection.recovery_sites[0].diagnostic_index, Some(0));
        assert_eq!(inspection.semantic.as_ref().unwrap().binding_names, ["broken", "intact"]);
    }

    #[test]
    fn normal_inspection_reports_aborted_program() {
        let inspection =
            inspect_recovery("const broken = ; const intact: number = 1;", &options("normal"))
                .unwrap();

        assert_eq!(inspection.status, "aborted");
        assert_eq!(inspection.statement_count, 0);
        assert_eq!(inspection.recovery_site_count, 0);
        assert!(inspection.declaration_names.is_empty());
        assert!(inspection.semantic.is_none());
    }

    #[test]
    fn assignment_inspection_reports_missing_rhs_and_reference() {
        let inspection =
            inspect_recovery("let target = 1; target = ; const intact = 2;", &options("editor"))
                .unwrap();

        assert_eq!(inspection.status, "recovered");
        assert_eq!(inspection.statement_count, 3);
        assert_eq!(inspection.recovery_site_count, 1);
        assert_eq!(inspection.recovery_sites[0].start, 25);
        assert_eq!(inspection.semantic.as_ref().unwrap().reference_count, 1);
    }

    #[test]
    fn object_property_inspection_reports_owned_recovery_path() {
        let inspection = inspect_recovery(
            "const value = { missing: , intact: 1 }; const later = 2;",
            &options("editor"),
        )
        .unwrap();

        assert_eq!(inspection.status, "recovered");
        assert_eq!(inspection.statement_count, 2);
        assert_eq!(inspection.recovery_site_count, 1);
        assert_eq!(inspection.recovery_sites[0].start, 25);
        assert_eq!(inspection.recovery_sites[0].parent_path.last(), Some(&"ObjectProperty"));
        assert_eq!(inspection.declaration_names, ["value", "later"]);
        assert_eq!(inspection.semantic.as_ref().unwrap().binding_names, ["later", "value"]);
    }

    #[test]
    fn array_inspection_reports_assignment_and_spread_recovery_sites() {
        let inspection = inspect_recovery(
            "let target = 1; const values = [target = , ... , 2]; const later = 3;",
            &options("editor"),
        )
        .unwrap();

        assert_eq!(inspection.status, "recovered");
        assert_eq!(inspection.statement_count, 3);
        assert_eq!(inspection.recovery_site_count, 2);
        assert_eq!(inspection.recovery_sites[0].start, 41);
        assert_eq!(inspection.recovery_sites[1].start, 47);
        assert_eq!(inspection.semantic.as_ref().unwrap().reference_count, 1);
        assert_eq!(
            inspection.semantic.as_ref().unwrap().binding_names,
            ["later", "target", "values"]
        );
    }

    #[test]
    fn call_inspection_reports_missing_argument_and_surviving_call_reference() {
        let inspection = inspect_recovery(
            "function f(a: number, b: number): void {} f(, 2); const later = 3;",
            &options("editor"),
        )
        .unwrap();

        assert_eq!(inspection.status, "recovered");
        assert_eq!(inspection.statement_count, 3);
        assert_eq!(inspection.recovery_site_count, 1);
        assert_eq!(inspection.diagnostics[0].message, "Argument expression expected.");
        assert_eq!(inspection.semantic.as_ref().unwrap().reference_count, 1);
        assert_eq!(inspection.semantic.as_ref().unwrap().binding_names, ["f", "later"]);
    }

    #[test]
    fn delimiter_inspection_reports_synthetic_token_recovery_sites() {
        let inspection = inspect_recovery(
            "const object = { first: 1 second: 2 }; const container = { values: [1, 2 }; const later = 3;",
            &options("editor"),
        )
        .unwrap();

        assert_eq!(inspection.status, "recovered");
        assert_eq!(inspection.statement_count, 3);
        assert_eq!(inspection.diagnostic_count, 2);
        assert_eq!(inspection.recovery_site_count, 2);
        assert_eq!(inspection.recovery_sites[0].kind, "MissingComma");
        assert_eq!(inspection.recovery_sites[0].parent_path, ["Program"]);
        assert_eq!(inspection.recovery_sites[0].diagnostic_index, Some(0));
        assert_eq!(inspection.recovery_sites[1].kind, "MissingClosingBracket");
        assert_eq!(inspection.recovery_sites[1].parent_path, ["Program"]);
        assert_eq!(inspection.recovery_sites[1].diagnostic_index, Some(1));
        assert_eq!(inspection.declaration_names, ["object", "container", "later"]);
        assert_eq!(
            inspection.semantic.as_ref().unwrap().binding_names,
            ["container", "later", "object"]
        );
    }

    #[test]
    fn parameter_inspection_reports_missing_comma_and_surviving_bindings() {
        let inspection = inspect_recovery(
            "function f(first: number second: string): void {} const later = 3;",
            &options("editor"),
        )
        .unwrap();

        assert_eq!(inspection.status, "recovered");
        assert_eq!(inspection.statement_count, 2);
        assert_eq!(inspection.diagnostic_count, 1);
        assert_eq!(inspection.recovery_site_count, 1);
        assert_eq!(inspection.recovery_sites[0].kind, "MissingComma");
        assert_eq!(inspection.recovery_sites[0].start, 25);
        assert_eq!(inspection.recovery_sites[0].diagnostic_index, Some(0));
        assert_eq!(inspection.declaration_names, ["f", "later"]);
        assert_eq!(inspection.semantic.as_ref().unwrap().binding_names, ["f", "later"]);
    }

    #[test]
    fn parameter_inspection_reports_missing_slot_without_an_invented_binding() {
        let inspection = inspect_recovery(
            "function broken(, second: number): void {} const later = 3;",
            &options("editor"),
        )
        .unwrap();

        assert_eq!(inspection.status, "recovered");
        assert_eq!(inspection.statement_count, 2);
        assert_eq!(inspection.diagnostic_count, 1);
        assert_eq!(inspection.recovery_site_count, 1);
        assert_eq!(inspection.recovery_sites[0].kind, "MissingParameter");
        assert_eq!(inspection.recovery_sites[0].start, 16);
        assert_eq!(inspection.recovery_sites[0].diagnostic_index, Some(0));
        assert_eq!(inspection.declaration_names, ["broken", "later"]);
        assert_eq!(inspection.semantic.as_ref().unwrap().binding_names, ["broken", "later"]);
    }

    #[test]
    fn function_body_inspection_reports_missing_closing_brace() {
        let inspection = inspect_recovery(
            "const prior = 1; function f(): number { return prior;",
            &options("editor"),
        )
        .unwrap();

        assert_eq!(inspection.status, "recovered");
        assert_eq!(inspection.statement_count, 2);
        assert_eq!(inspection.diagnostic_count, 1);
        assert_eq!(inspection.recovery_site_count, 1);
        assert_eq!(inspection.recovery_sites[0].kind, "MissingClosingBrace");
        assert_eq!(inspection.recovery_sites[0].diagnostic_index, Some(0));
        assert_eq!(inspection.declaration_names, ["prior", "f"]);
        assert_eq!(inspection.semantic.as_ref().unwrap().binding_names, ["f", "prior"]);
        assert_eq!(inspection.semantic.as_ref().unwrap().reference_count, 1);
    }

    #[test]
    fn return_expression_inspection_reports_missing_binary_operand() {
        let inspection = inspect_recovery(
            "function f(): number { return 1 + } const later = 2;",
            &options("editor"),
        )
        .unwrap();

        assert_eq!(inspection.status, "recovered");
        assert_eq!(inspection.statement_count, 2);
        assert_eq!(inspection.diagnostic_count, 1);
        assert_eq!(inspection.recovery_site_count, 1);
        assert_eq!(inspection.recovery_sites[0].kind, "MissingExpression");
        assert_eq!(inspection.recovery_sites[0].start, 34);
        assert_eq!(inspection.recovery_sites[0].diagnostic_index, Some(0));
        assert_eq!(inspection.declaration_names, ["f", "later"]);
        assert_eq!(inspection.semantic.as_ref().unwrap().binding_names, ["f", "later"]);
    }

    #[test]
    fn interface_inspection_reports_missing_closing_brace() {
        let inspection =
            inspect_recovery("const prior = 1; interface Box { value: number;", &options("editor"))
                .unwrap();

        assert_eq!(inspection.status, "recovered");
        assert_eq!(inspection.statement_count, 2);
        assert_eq!(inspection.diagnostic_count, 1);
        assert_eq!(inspection.recovery_site_count, 1);
        assert_eq!(inspection.recovery_sites[0].kind, "MissingClosingBrace");
        assert_eq!(inspection.recovery_sites[0].diagnostic_index, Some(0));
        assert_eq!(inspection.declaration_names, ["prior", "Box"]);
        assert_eq!(inspection.semantic.as_ref().unwrap().binding_names, ["Box", "prior"]);
    }

    #[test]
    fn interface_inspection_reports_missing_member_separator() {
        let inspection = inspect_recovery(
            "interface Box { first: number second: string } const later = 2;",
            &options("editor"),
        )
        .unwrap();

        assert_eq!(inspection.status, "recovered");
        assert_eq!(inspection.statement_count, 2);
        assert_eq!(inspection.diagnostic_count, 1);
        assert_eq!(inspection.recovery_site_count, 1);
        assert_eq!(inspection.recovery_sites[0].kind, "MissingSemicolon");
        assert_eq!(inspection.recovery_sites[0].start, 30);
        assert_eq!(inspection.recovery_sites[0].diagnostic_index, Some(0));
        assert_eq!(inspection.declaration_names, ["Box", "later"]);
        assert_eq!(inspection.semantic.as_ref().unwrap().binding_names, ["Box", "later"]);
    }

    #[test]
    fn class_inspection_reports_missing_member_separator() {
        let source_text =
            "class Box { first: number = 1 second: string = \"ok\"; } const later = 2;";
        let offset = u32::try_from(source_text.find("second").expect("fixture contains second"))
            .expect("source offset fits in u32");
        let inspection = inspect_recovery(source_text, &options("editor")).unwrap();

        assert_eq!(inspection.status, "recovered");
        assert_eq!(inspection.statement_count, 2);
        assert_eq!(inspection.diagnostic_count, 1);
        assert_eq!(inspection.recovery_site_count, 1);
        assert_eq!(inspection.recovery_sites[0].kind, "MissingSemicolon");
        assert_eq!(inspection.recovery_sites[0].start, offset);
        assert_eq!(inspection.recovery_sites[0].diagnostic_index, Some(0));
        assert_eq!(inspection.declaration_names, ["Box", "later"]);
        assert_eq!(inspection.semantic.as_ref().unwrap().binding_names, ["Box", "later"]);
    }

    #[test]
    fn class_inspection_reports_missing_closing_brace() {
        let source_text = "const prior = 1; class Box { value: number = 1;";
        let offset = u32::try_from(source_text.len()).expect("source length fits in u32");
        let inspection = inspect_recovery(source_text, &options("editor")).unwrap();

        assert_eq!(inspection.status, "recovered");
        assert_eq!(inspection.statement_count, 2);
        assert_eq!(inspection.diagnostic_count, 1);
        assert_eq!(inspection.recovery_site_count, 1);
        assert_eq!(inspection.recovery_sites[0].kind, "MissingClosingBrace");
        assert_eq!(inspection.recovery_sites[0].start, offset);
        assert_eq!(inspection.recovery_sites[0].diagnostic_index, Some(0));
        assert_eq!(inspection.declaration_names, ["prior", "Box"]);
        assert_eq!(inspection.semantic.as_ref().unwrap().binding_names, ["Box", "prior"]);
    }

    #[test]
    fn call_inspection_reports_a_closer_owned_by_a_following_declaration() {
        let inspection = inspect_recovery(
            "function check(value: number): void {}\ncheck(\nconst later = 2;",
            &options("editor"),
        )
        .unwrap();

        assert_eq!(inspection.status, "recovered");
        assert_eq!(inspection.statement_count, 3);
        assert_eq!(inspection.diagnostic_count, 1);
        assert_eq!(inspection.recovery_site_count, 1);
        assert_eq!(inspection.recovery_sites[0].kind, "MissingClosingParenthesis");
        assert_eq!(inspection.recovery_sites[0].start, 46);
        assert_eq!(inspection.recovery_sites[0].diagnostic_index, Some(0));
        assert_eq!(inspection.declaration_names, ["check", "later"]);
        assert_eq!(inspection.semantic.as_ref().unwrap().binding_names, ["check", "later"]);
    }

    #[test]
    fn declaration_inspection_reports_a_missing_name_without_a_binding() {
        let inspection =
            inspect_recovery("const = 1;\nconst later = 2;", &options("editor")).unwrap();

        assert_eq!(inspection.status, "recovered");
        assert_eq!(inspection.statement_count, 3);
        assert_eq!(inspection.diagnostic_count, 2);
        assert_eq!(inspection.recovery_site_count, 2);
        assert_eq!(inspection.recovery_sites[0].kind, "MissingDeclarationName");
        assert_eq!(inspection.recovery_sites[0].start, 6);
        assert_eq!(inspection.recovery_sites[0].diagnostic_index, Some(0));
        assert_eq!(inspection.recovery_sites[1].kind, "UnexpectedVariableInitializer");
        assert_eq!(inspection.recovery_sites[1].start, 8);
        assert_eq!(inspection.recovery_sites[1].diagnostic_index, Some(1));
        assert_eq!(inspection.declaration_names, ["later"]);
        assert_eq!(inspection.semantic.as_ref().unwrap().binding_names, ["later"]);
    }

    #[test]
    fn member_inspection_reports_zero_width_missing_property_and_object_reference() {
        let source_text =
            "interface Box { value: number; } declare const box: Box; box.; const later = 2;";
        let missing_offset =
            u32::try_from(source_text.find(".;").expect("missing member access") + 1)
                .expect("source offset fits in u32");
        let inspection = inspect_recovery(source_text, &options("editor")).unwrap();

        assert_eq!(inspection.status, "recovered");
        assert_eq!(inspection.statement_count, 4);
        assert_eq!(inspection.diagnostic_count, 1);
        assert_eq!(inspection.recovery_site_count, 1);
        assert_eq!(inspection.recovery_sites[0].kind, "MissingMemberExpression");
        assert_eq!(inspection.recovery_sites[0].start, missing_offset);
        assert_eq!(inspection.recovery_sites[0].end, missing_offset);
        assert_eq!(inspection.recovery_sites[0].diagnostic_index, Some(0));
        assert_eq!(inspection.declaration_names, ["Box", "box", "later"]);
        assert_eq!(inspection.semantic.as_ref().unwrap().binding_names, ["Box", "box", "later"]);
        assert_eq!(inspection.semantic.as_ref().unwrap().reference_count, 2);
    }

    #[test]
    fn missing_type_inspection_reports_recovered_type_nodes() {
        let inspection = inspect_recovery(
            "type Broken = ; const value: = 1; const intact: number = 2;",
            &options("editor"),
        )
        .unwrap();

        assert_eq!(inspection.status, "recovered");
        assert_eq!(inspection.statement_count, 3);
        assert_eq!(inspection.diagnostic_count, 2);
        assert_eq!(inspection.recovery_site_count, 2);
        assert!(inspection.recovery_sites.iter().all(|site| site.kind == "MissingType"));
        assert_eq!(inspection.recovery_sites[0].diagnostic_index, Some(0));
        assert_eq!(inspection.recovery_sites[1].diagnostic_index, Some(1));
        assert_eq!(inspection.declaration_names, ["Broken", "value", "intact"]);
        let bindings = &inspection.semantic.as_ref().unwrap().binding_names;
        assert!(bindings.iter().any(|name| name == "value"));
        assert!(bindings.iter().any(|name| name == "intact"));
    }

    #[test]
    fn malformed_expression_inspection_reports_source_backed_recovery_nodes() {
        let inspection = inspect_recovery(
            "const broken: number = :; const intact: number = 2;",
            &options("editor"),
        )
        .unwrap();

        assert_eq!(inspection.status, "recovered");
        assert_eq!(inspection.statement_count, 2);
        assert_eq!(inspection.diagnostic_count, 1);
        assert_eq!(inspection.recovery_site_count, 1);
        assert_eq!(inspection.recovery_sites[0].kind, "MalformedExpression");
        assert_eq!(inspection.recovery_sites[0].start, 23);
        assert_eq!(inspection.recovery_sites[0].end, 24);
        assert_eq!(inspection.recovery_sites[0].diagnostic_index, Some(0));
        assert_eq!(inspection.declaration_names, ["broken", "intact"]);
        assert_eq!(inspection.semantic.as_ref().unwrap().binding_names, ["broken", "intact"]);
    }

    #[test]
    fn valid_editor_inspection_is_clean() {
        let inspection =
            inspect_recovery("let target = 1; target = 2;", &options("editor")).unwrap();
        assert_eq!(inspection.status, "clean");
        assert_eq!(inspection.diagnostic_count, 0);
        assert_eq!(inspection.recovery_site_count, 0);
    }
}
