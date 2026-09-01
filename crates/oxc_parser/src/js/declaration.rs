use oxc_allocator::{ArenaBox, ArenaVec};
use oxc_ast::ast::*;
use oxc_data_structures::branch_hints::unlikely;
use oxc_span::{GetSpan, Span};

use super::VariableDeclarationParent;
use crate::{
    ParseMode, ParserConfig as Config, ParserImpl, RecoveryContext, StatementContext, diagnostics,
    lexer::Kind,
};

impl<'a, C: Config> ParserImpl<'a, C> {
    pub(crate) fn parse_let(&mut self, stmt_ctx: StatementContext) -> Statement<'a> {
        let start = self.cur_start();

        let peeked = self.lexer.peek_token().kind();

        // Fast path: avoid rewind.
        if !stmt_ctx.is_single_statement() && peeked.is_after_let() {
            self.bump_any(); // bump `let`
            return self.parse_variable_statement(start, VariableDeclarationKind::Let, stmt_ctx);
        }

        // let = foo, let instanceof x, let + 1
        if peeked.is_assignment_operator() || peeked.is_binary_operator() {
            let expr = self.parse_assignment_expression_or_higher();
            self.parse_expression_statement(start, expr)
        // let.a = 1, let?.a = 1, let()[a] = 1
        } else if matches!(peeked, Kind::Dot | Kind::QuestionDot | Kind::LParen) {
            let expr = self.parse_expr();
            self.parse_expression_statement(start, expr)
        // single statement let declaration: while (0) let
        } else if (stmt_ctx.is_single_statement() && peeked != Kind::LBrack)
            || peeked == Kind::Semicolon
        {
            let expr = self.parse_identifier_expression();
            self.parse_expression_statement(start, expr)
        } else {
            self.bump_any();
            self.parse_variable_statement(start, VariableDeclarationKind::Let, stmt_ctx)
        }
    }

    pub(crate) fn is_using_statement(&mut self) -> bool {
        // `await using` requires `using` immediately after `await` on the same line. Cheaply peek
        // for it first, so the common `await <expr>` statement avoids the heavier `lookahead`
        // (checkpoint + rewind) and only `await using` pays for the binding-identifier check.
        let next = self.lexer.peek_token();
        next.kind() == Kind::Using
            && !next.is_on_new_line()
            && self.lookahead(Self::is_next_token_using_keyword_then_binding_identifier)
    }

    fn is_next_token_using_keyword_then_binding_identifier(&mut self) -> bool {
        self.bump_any();
        if !self.cur_token().is_on_new_line() && self.eat(Kind::Using) {
            self.cur_kind().is_binding_identifier() && !self.cur_token().is_on_new_line()
        } else {
            false
        }
    }

    pub(crate) fn parse_using_statement(&mut self, stmt_ctx: StatementContext) -> Statement<'a> {
        let mut decl = self.parse_using_declaration(stmt_ctx);
        self.asi();
        decl.span = self.end_span(decl.span.start);
        debug_assert!(decl.kind.is_lexical());
        if stmt_ctx.is_single_statement() {
            self.error(diagnostics::lexical_declaration_single_statement(decl.span));
        }
        Statement::VariableDeclaration(decl)
    }

    pub(crate) fn get_variable_declaration_kind(&self) -> VariableDeclarationKind {
        match self.cur_kind() {
            Kind::Var => VariableDeclarationKind::Var,
            Kind::Const => VariableDeclarationKind::Const,
            Kind::Let => VariableDeclarationKind::Let,
            _ => unreachable!(),
        }
    }

    pub(crate) fn parse_variable_declaration(
        &mut self,
        start: u32,
        kind: VariableDeclarationKind,
        decl_parent: VariableDeclarationParent,
        declare: bool,
    ) -> ArenaBox<'a, VariableDeclaration<'a>> {
        let mut declarations = ArenaVec::new_in(self);
        if unlikely(self.at(Kind::Eq))
            && self.options.mode == ParseMode::Editor
            && decl_parent == VariableDeclarationParent::Statement
        {
            let missing_name_span = self.cur_token().span();
            self.error(diagnostics::variable_declaration_expected(missing_name_span));
            self.record_recovery("MissingDeclarationName", missing_name_span);
            self.bump_any();

            let initializer_span = self.cur_token().span();
            self.error(diagnostics::variable_declaration_expected(initializer_span));
            self.record_recovery("UnexpectedVariableInitializer", initializer_span);

            return VariableDeclaration::boxed(
                self.end_span(start),
                kind,
                declarations,
                declare,
                self,
            );
        }

        self.recovery_context_add(RecoveryContext::VariableDeclarations, |parser| {
            let mut recovered_declarator = false;
            loop {
                let declaration = parser.parse_variable_declarator(decl_parent, kind);
                let invalid_numeric_suffix = declaration.init.as_ref().is_some_and(|initializer| {
                    matches!(initializer, Expression::NumericLiteral(_))
                        && initializer.span().end == parser.cur_start()
                });
                let recover_missing_separator = parser.options.mode == ParseMode::Editor
                    && !parser.cur_token().is_on_new_line()
                    && parser.cur_kind().is_binding_identifier();

                if decl_parent == VariableDeclarationParent::Statement
                    && !recovered_declarator
                    && !recover_missing_separator
                {
                    parser.check_missing_initializer(&declaration, kind);
                }
                declarations.push(declaration);
                if parser.eat(Kind::Comma) {
                    recovered_declarator = false;
                    continue;
                }
                if recover_missing_separator {
                    let token_span = parser.cur_token().span();
                    if invalid_numeric_suffix {
                        parser.record_recovery("InvalidNumericSuffix", token_span);
                    } else {
                        parser.record_missing_token_recovery(
                            Kind::Comma,
                            Span::empty(token_span.start),
                        );
                        parser.error(diagnostics::typescript_expected_token(",", token_span));
                    }
                    recovered_declarator = true;
                    continue;
                }
                break;
            }
        });

        if matches!(decl_parent, VariableDeclarationParent::Statement) {
            let recovered_before_statement = self.options.mode == ParseMode::Editor
                && declarations.last().is_some_and(|declaration| {
                    matches!(declaration.init, Some(Expression::MissingExpression(_)))
                })
                && self.at_recovery_statement_element_start();
            if !recovered_before_statement {
                self.asi();
            }
        }
        VariableDeclaration::boxed(self.end_span(start), kind, declarations, declare, self)
    }

    fn parse_variable_declarator(
        &mut self,
        decl_parent: VariableDeclarationParent,
        kind: VariableDeclarationKind,
    ) -> VariableDeclarator<'a> {
        let start = self.cur_start();

        let id = self.parse_binding_pattern();

        let (type_annotation, definite_start) = if self.is_ts {
            // const x!: number = 1
            //        ^ definite
            let definite_start = if id.is_binding_identifier()
                && !self.cur_token().is_on_new_line()
                && self.at(Kind::Bang)
            {
                let definite_start = self.cur_token().start();
                self.bump_any();
                Some(definite_start)
            } else {
                None
            };
            if self.at(Kind::Question) {
                self.error(diagnostics::unexpected_optional_declaration(self.cur_token().span()));
                self.bump_any();
            }
            let type_annotation = self.parse_ts_type_annotation();
            (type_annotation, definite_start)
        } else {
            (None, None)
        };
        let init = if !self.eat(Kind::Eq) {
            None
        } else if unlikely(self.options.mode == ParseMode::Editor)
            && self.at_recovery_context_boundary()
            && (decl_parent == VariableDeclarationParent::Statement
                || !self.at_recovery_statement_element_start())
        {
            let diagnostic_span = Span::empty(self.cur_start());
            // A following statement is the recovery boundary, not part of the missing initializer.
            // Keep the missing node at the end of `=` while leaving the diagnostic free to point at
            // the unexpected statement token, matching TypeScript's recovered tree.
            let missing_start = if decl_parent == VariableDeclarationParent::Statement
                && self.at_recovery_statement_element_start()
            {
                self.prev_token_end
            } else {
                self.cur_start()
            };
            let span = Span::empty(missing_start);
            self.error(diagnostics::expression_expected(diagnostic_span));
            Some(Expression::MissingExpression(MissingExpression::boxed(span, self)))
        } else {
            Some(self.parse_assignment_expression_or_higher())
        };
        let decl = VariableDeclarator::new(
            self.end_span(start),
            id,
            type_annotation,
            init,
            definite_start.is_some(),
            self,
        );
        if self.ctx.has_ambient()
            && let Some(init) = &decl.init
            && !kind.is_using()
            && !(kind.is_const() && decl.type_annotation.is_none())
        {
            self.error(diagnostics::initializers_not_allowed_in_ambient_contexts(init.span()));
        }
        if let Some(definite_start) = definite_start {
            let span = Span::sized(definite_start, 1);
            if decl.init.is_some() {
                self.error(diagnostics::variable_declarator_definite(span));
            } else if decl.type_annotation.is_none() {
                self.error(diagnostics::variable_declarator_definite_type_assertion(span));
            } else if self.ctx.has_ambient() {
                self.error(diagnostics::definite_assignment_assertion_not_permitted(span));
            }
        }
        decl
    }

    pub(crate) fn check_missing_initializer(
        &mut self,
        decl: &VariableDeclarator<'a>,
        kind: VariableDeclarationKind,
    ) {
        if decl.init.is_none() && !self.ctx.has_ambient() {
            if !matches!(decl.id, BindingPattern::BindingIdentifier(_)) {
                self.error(diagnostics::invalid_destructuring_declaration(decl.id.span()));
            } else if kind == VariableDeclarationKind::Const {
                // It is a Syntax Error if Initializer is not present and IsConstantDeclaration of the LexicalDeclaration containing this LexicalBinding is true.
                self.error(diagnostics::missing_initializer_in_const(decl.id.span()));
            } else if kind.is_using() {
                self.error(diagnostics::using_declarations_must_be_initialized(decl.id.span()));
            }
        }
    }

    /// Section 14.3.1 Let, Const, and Using Declarations
    /// UsingDeclaration[In, Yield, Await] :
    /// using [no LineTerminator here] [lookahead ≠ await] BindingList[?In, ?Yield, ?Await, ~Pattern] ;
    pub(crate) fn parse_using_declaration(
        &mut self,
        statement_ctx: StatementContext,
    ) -> ArenaBox<'a, VariableDeclaration<'a>> {
        let start = self.cur_start();

        let is_await = self.eat(Kind::Await);
        let kind = if is_await {
            VariableDeclarationKind::AwaitUsing
        } else {
            VariableDeclarationKind::Using
        };

        self.expect(Kind::Using);
        if self.ctx.has_ambient() {
            let using_span = self.cur_token().span();
            self.error(if kind.is_await() {
                diagnostics::await_using_declarations_not_allowed_in_ambient_contexts(using_span)
            } else {
                diagnostics::using_declarations_not_allowed_in_ambient_contexts(using_span)
            });
        }

        // BindingList[?In, ?Yield, ?Await, ~Pattern]
        let mut declarations = ArenaVec::new_in(self);
        loop {
            let decl_parent = if matches!(statement_ctx, StatementContext::For) {
                VariableDeclarationParent::For
            } else {
                VariableDeclarationParent::Statement
            };
            let declaration = self.parse_variable_declarator(decl_parent, kind);
            if decl_parent == VariableDeclarationParent::Statement {
                self.check_missing_initializer(&declaration, kind);
            }

            if !matches!(declaration.id, BindingPattern::BindingIdentifier(_)) {
                self.error(diagnostics::invalid_identifier_in_using_declaration(
                    declaration.id.span(),
                ));
            }

            declarations.push(declaration);
            if !self.eat(Kind::Comma) {
                break;
            }
        }

        VariableDeclaration::boxed(self.end_span(start), kind, declarations, false, self)
    }
}
