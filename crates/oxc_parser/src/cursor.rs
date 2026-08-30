//! Code related to navigating `Token`s from the lexer

use oxc_allocator::{ArenaBox, ArenaVec};
use oxc_ast::ast::{BindingRestElement, RegExpFlags};
use oxc_data_structures::branch_hints::unlikely;
use oxc_span::{GetSpan, Span};

use crate::{
    Context, ParseMode, ParserConfig as Config, ParserImpl, RecoveryContext, RecoveryEvent,
    diagnostics::{self, ParserDiagnostic},
    error_handler::FatalError,
    lexer::{Kind, LexerCheckpoint, Token, cold_branch},
};

#[derive(Clone)]
pub struct ParserCheckpoint<'a> {
    lexer: LexerCheckpoint<'a>,
    cur_token: Token,
    prev_span_end: u32,
    errors_pos: usize,
    recoveries_pos: usize,
    fatal_error: Option<FatalError<'a>>,
}

impl<'a, C: Config> ParserImpl<'a, C> {
    pub(crate) fn record_recovery(&mut self, kind: &'static str, span: Span) {
        self.recoveries.push(RecoveryEvent { kind, span });
    }

    pub(crate) fn record_missing_token_recovery(&mut self, kind: Kind, span: Span) {
        let kind = match kind {
            Kind::Comma => "MissingComma",
            Kind::RCurly => "MissingClosingBrace",
            Kind::RBrack => "MissingClosingBracket",
            Kind::RParen => "MissingClosingParenthesis",
            Kind::RAngle => "MissingClosingAngleBracket",
            _ => "MissingToken",
        };
        self.record_recovery(kind, span);
    }

    fn last_recovery_is_missing_token(&self, kind: Kind, span: Span) -> bool {
        let expected_kind = match kind {
            Kind::Comma => "MissingComma",
            Kind::RCurly => "MissingClosingBrace",
            Kind::RBrack => "MissingClosingBracket",
            Kind::RParen => "MissingClosingParenthesis",
            Kind::RAngle => "MissingClosingAngleBracket",
            _ => "MissingToken",
        };
        self.recoveries
            .last()
            .is_some_and(|recovery| recovery.kind == expected_kind && recovery.span == span)
    }

    /// Get current token's span start.
    #[inline]
    pub(crate) fn cur_start(&self) -> u32 {
        self.token.start()
    }

    /// Create a [`Span`] from provided `start` to end of previous token.
    #[inline]
    pub(crate) fn end_span(&self, start: u32) -> Span {
        Span::new(start, self.prev_token_end)
    }

    /// Get current token
    #[inline]
    pub(crate) fn cur_token(&self) -> Token {
        self.token
    }

    /// Get current Kind
    #[inline]
    pub(crate) fn cur_kind(&self) -> Kind {
        self.token.kind()
    }

    /// Get current source text
    #[inline]
    pub(crate) fn cur_src(&self) -> &'a str {
        self.token_source(&self.token)
    }

    /// Get source text for a token
    #[inline]
    pub(crate) fn token_source(&self, token: &Token) -> &'a str {
        let span = token.span();
        if cfg!(debug_assertions) {
            &self.source_text[span.start as usize..span.end as usize]
        } else {
            // SAFETY:
            // Span comes from the lexer, which ensures:
            // * `start` and `end` are in bounds of source text.
            // * `end >= start`.
            // * `start` and `end` are both on UTF-8 char boundaries.
            // * `self.source_text` is same text that `Token`s are generated from.
            //
            // TODO: I (@overlookmotel) don't think we should really be doing this.
            // We don't have static guarantees of these properties.
            unsafe { self.source_text.get_unchecked(span.start as usize..span.end as usize) }
        }
    }

    /// Get current string
    pub(crate) fn cur_string(&self) -> &'a str {
        self.lexer.get_string(self.token)
    }

    /// Get current template string
    pub(crate) fn cur_template_string(&self) -> Option<&'a str> {
        self.lexer.get_template_string(self.token.start())
    }

    /// Checks if the current index has token `Kind`
    #[inline]
    pub(crate) fn at(&self, kind: Kind) -> bool {
        self.cur_kind() == kind
    }

    /// `StringValue` of `IdentifierName` normalizes any Unicode escape sequences
    /// in `IdentifierName` hence such escapes cannot be used to write an Identifier
    /// whose code point sequence is the same as a `ReservedWord`.
    #[cold]
    fn report_escaped_keyword(&mut self, span: Span) {
        self.error(diagnostics::escaped_keyword(span));
    }

    /// Move to the next token
    /// Checks if the current token is escaped if it is a keyword
    #[inline]
    pub(crate) fn advance(&mut self, kind: Kind) {
        // Manually inlined escaped keyword check - escaped identifiers are extremely rare
        if self.token.escaped() && kind.is_any_keyword() {
            self.report_escaped_keyword(self.token.span());
        }
        self.prev_token_end = self.token.end();
        self.token = self.lexer.next_token();
    }

    /// Move to the next `JSXChild`
    /// Checks if the current token is escaped if it is a keyword
    pub(crate) fn advance_for_jsx_child(&mut self) {
        self.prev_token_end = self.token.end();
        self.token = self.lexer.next_jsx_child();
    }

    /// Advance and return true if we are at `Kind`, return false otherwise
    #[inline]
    #[must_use = "Use `bump` instead of `eat` if you are ignoring the return value"]
    pub(crate) fn eat(&mut self, kind: Kind) -> bool {
        if self.at(kind) {
            self.advance(kind);
            return true;
        }
        false
    }

    /// Advance if we are at `Kind`
    #[inline]
    pub(crate) fn bump(&mut self, kind: Kind) {
        if self.at(kind) {
            self.advance(kind);
        }
    }

    /// Advance any token
    #[inline]
    pub(crate) fn bump_any(&mut self) {
        self.advance(self.cur_kind());
    }

    /// [Automatic Semicolon Insertion](https://tc39.es/ecma262/#sec-automatic-semicolon-insertion)
    /// # Errors
    pub(crate) fn asi(&mut self) {
        if self.eat(Kind::Semicolon) || self.can_insert_semicolon() {
            /* no op */
        } else {
            // ASI failure is a syntax error (cold). Build the diagnostic out of line so the
            // ~232-byte `OxcDiagnostic` buffer does not inflate `asi`'s stack frame on the
            // common (valid) path.
            cold_branch(|| {
                let span = Span::empty(self.prev_token_end);
                let error = diagnostics::auto_semicolon_insertion(span);
                self.set_fatal_error(error);
            });
        }
    }

    #[inline]
    pub(crate) fn can_insert_semicolon(&self) -> bool {
        let token = self.cur_token();
        matches!(token.kind(), Kind::Semicolon | Kind::RCurly | Kind::Eof) || token.is_on_new_line()
    }

    /// Cold path for expect failures - separated to improve branch prediction
    #[cold]
    #[inline(never)]
    fn handle_expect_failure(&mut self, expected_kind: Kind) {
        let range = self.cur_token().span();
        let error =
            diagnostics::expect_token(expected_kind.to_str(), self.cur_kind().to_str(), range);
        self.set_fatal_error(error);
    }

    /// # Errors
    #[inline]
    pub(crate) fn expect_without_advance(&mut self, kind: Kind) {
        if !self.at(kind) {
            self.handle_expect_failure(kind);
        }
    }

    /// Expect a `Kind` or return error
    /// # Errors
    #[inline]
    pub(crate) fn expect(&mut self, kind: Kind) {
        if !self.at(kind) {
            self.handle_expect_failure(kind);
        }
        self.advance(kind);
    }

    #[inline]
    pub(crate) fn expect_closing(&mut self, kind: Kind, opening_span: Span) {
        if !self.at(kind) {
            let range = self.cur_token().span();
            let error = diagnostics::expect_closing(
                kind.to_str(),
                self.cur_kind().to_str(),
                range,
                opening_span,
            );
            self.set_fatal_error(error);
        }
        self.advance(kind);
    }

    #[inline]
    pub(crate) fn expect_conditional_alternative(&mut self, question_span: Span) {
        if !self.at(Kind::Colon) {
            let range = self.cur_token().span();
            let error = diagnostics::expect_conditional_alternative(
                self.cur_kind().to_str(),
                range,
                question_span,
            );
            self.set_fatal_error(error);
        }
        self.bump_any(); // bump `:`
    }

    /// Expect the next next token to be a `JsxChild`, i.e. `<` or `{` or `JSXText`
    /// # Errors
    pub(crate) fn expect_jsx_child(&mut self, kind: Kind) {
        self.expect_without_advance(kind);
        self.advance_for_jsx_child();
    }

    /// Move to the next token, lexing it as a JSX attribute value.
    pub(crate) fn advance_for_jsx_attribute_value(&mut self) {
        self.prev_token_end = self.token.end();
        self.token = self.lexer.next_jsx_attribute_value();
    }

    /// Tell lexer to read a regex
    pub(crate) fn read_regex(&mut self) -> (u32, RegExpFlags, bool) {
        let (token, pattern_end, flags, flags_error) = self.lexer.next_regex(self.cur_kind());
        self.token = token;
        (pattern_end, flags, flags_error)
    }

    /// Tell lexer to read a template substitution tail
    pub(crate) fn re_lex_template_substitution_tail(&mut self) {
        if self.at(Kind::RCurly) {
            self.token = self.lexer.next_template_substitution_tail();
        }
    }

    /// Tell lexer to continue reading jsx identifier if the lexer character position is at `-` for `<component-name>`.
    ///
    /// Returns `true` if was continued.
    pub(crate) fn continue_lex_jsx_identifier(&mut self) -> bool {
        if let Some(token) = self.lexer.continue_lex_jsx_identifier(self.token.start()) {
            self.token = token;
            true
        } else {
            false
        }
    }

    #[inline]
    pub(crate) fn re_lex_right_angle(&mut self) -> Kind {
        if self.fatal_error.is_some() {
            return Kind::Eof;
        }
        let kind = self.cur_kind();
        if kind == Kind::RAngle {
            self.token = self.lexer.re_lex_right_angle();
            self.token.kind()
        } else {
            kind
        }
    }

    pub(crate) fn re_lex_ts_l_angle(&mut self) -> bool {
        if self.fatal_error.is_some() {
            return false;
        }
        let kind = self.cur_kind();
        if kind == Kind::ShiftLeft || kind == Kind::LtEq {
            self.token = self.lexer.re_lex_as_typescript_l_angle(2);
            true
        } else if kind == Kind::ShiftLeftEq {
            self.token = self.lexer.re_lex_as_typescript_l_angle(3);
            true
        } else {
            kind == Kind::LAngle
        }
    }

    pub(crate) fn re_lex_ts_r_angle(&mut self) -> bool {
        if self.fatal_error.is_some() {
            return false;
        }
        let kind = self.cur_kind();
        if kind == Kind::ShiftRight {
            self.token = self.lexer.re_lex_as_typescript_r_angle(2);
            true
        } else if kind == Kind::ShiftRight3 {
            self.token = self.lexer.re_lex_as_typescript_r_angle(3);
            true
        } else {
            kind == Kind::RAngle
        }
    }

    pub(crate) fn checkpoint(&mut self) -> ParserCheckpoint<'a> {
        ParserCheckpoint {
            lexer: self.lexer.checkpoint(),
            cur_token: self.token,
            prev_span_end: self.prev_token_end,
            errors_pos: self.errors.len(),
            recoveries_pos: self.recoveries.len(),
            fatal_error: self.fatal_error.take(),
        }
    }

    pub(crate) fn checkpoint_with_error_recovery(&mut self) -> ParserCheckpoint<'a> {
        ParserCheckpoint {
            lexer: self.lexer.checkpoint_with_error_recovery(),
            cur_token: self.token,
            prev_span_end: self.prev_token_end,
            errors_pos: self.errors.len(),
            recoveries_pos: self.recoveries.len(),
            fatal_error: self.fatal_error.take(),
        }
    }

    pub(crate) fn rewind(&mut self, checkpoint: ParserCheckpoint<'a>) {
        let ParserCheckpoint {
            lexer,
            cur_token,
            prev_span_end,
            errors_pos,
            recoveries_pos,
            fatal_error,
        } = checkpoint;

        self.lexer.rewind(lexer);
        self.token = cur_token;
        self.prev_token_end = prev_span_end;
        self.errors.truncate(errors_pos);
        self.recoveries.truncate(recoveries_pos);
        self.fatal_error = fatal_error;
    }

    pub(crate) fn lookahead<U>(&mut self, predicate: impl Fn(&mut ParserImpl<'a, C>) -> U) -> U {
        let checkpoint = self.checkpoint();
        let answer = predicate(self);
        self.rewind(checkpoint);
        answer
    }

    #[expect(clippy::inline_always)]
    #[inline(always)] // inline because this is always on a hot path
    pub(crate) fn context_add<F, T>(&mut self, add_flags: Context, cb: F) -> T
    where
        F: FnOnce(&mut Self) -> T,
    {
        let ctx = self.ctx;
        self.ctx = ctx.union(add_flags);
        let result = cb(self);
        self.ctx = ctx;
        result
    }

    #[expect(clippy::inline_always)]
    #[inline(always)] // inline because this is always on a hot path
    pub(crate) fn context_remove<F, T>(&mut self, remove_flags: Context, cb: F) -> T
    where
        F: FnOnce(&mut Self) -> T,
    {
        let ctx = self.ctx;
        self.ctx = ctx.difference(remove_flags);
        let result = cb(self);
        self.ctx = ctx;
        result
    }

    #[expect(clippy::inline_always)]
    #[inline(always)] // inline because this is always on a hot path
    pub(crate) fn context<F, T>(&mut self, add_flags: Context, remove_flags: Context, cb: F) -> T
    where
        F: FnOnce(&mut Self) -> T,
    {
        let ctx = self.ctx;
        self.ctx = ctx.difference(remove_flags).union(add_flags);
        let result = cb(self);
        self.ctx = ctx;
        result
    }

    /// Add an editor-recovery list context for the duration of `cb`.
    #[expect(clippy::inline_always)]
    #[inline(always)]
    pub(crate) fn recovery_context_add<F, T>(&mut self, context: RecoveryContext, cb: F) -> T
    where
        F: FnOnce(&mut Self) -> T,
    {
        let previous = self.recovery_ctx;
        self.recovery_ctx.insert(context);
        let result = cb(self);
        self.recovery_ctx = previous;
        result
    }

    pub(crate) fn parse_normal_list<F, T>(
        &mut self,
        open: Kind,
        close: Kind,
        f: F,
    ) -> ArenaVec<'a, T>
    where
        F: FnMut(&mut Self) -> T,
    {
        let opening_span = self.cur_token().span();
        self.expect(open);
        let mut list = ArenaVec::new_in(self);
        self.parse_normal_list_into(&mut list, close, f);
        self.expect_closing(close, opening_span);
        list
    }

    pub(crate) fn parse_recoverable_normal_list<F, T>(
        &mut self,
        context: RecoveryContext,
        open: Kind,
        close: Kind,
        mut parse_element: F,
    ) -> ArenaVec<'a, T>
    where
        F: FnMut(&mut Self) -> T,
    {
        let opening_span = self.cur_token().span();
        self.expect(open);
        let mut list = ArenaVec::new_in(self);
        self.recovery_context_add(context, |parser| {
            parser.parse_normal_list_into(&mut list, close, &mut parse_element);
        });
        self.expect_recoverable_closing(close, opening_span, context);
        list
    }

    #[expect(clippy::inline_always)]
    #[inline(always)]
    fn parse_normal_list_into<F, T>(
        &mut self,
        list: &mut ArenaVec<'a, T>,
        close: Kind,
        mut parse_element: F,
    ) where
        F: FnMut(&mut Self) -> T,
    {
        loop {
            let kind = self.cur_kind();
            if kind == close
                || matches!(kind, Kind::Eof | Kind::Undetermined)
                || self.fatal_error.is_some()
            {
                break;
            }
            let element = parse_element(self);
            list.push(element);
        }
    }

    pub(crate) fn parse_recoverable_normal_list_breakable<F, T>(
        &mut self,
        context: RecoveryContext,
        open: Kind,
        close: Kind,
        parse_element: F,
    ) -> ArenaVec<'a, T>
    where
        F: Fn(&mut Self) -> Option<T>,
    {
        let opening_span = self.cur_token().span();
        self.expect(open);
        let mut list = ArenaVec::new_in(self);
        self.recovery_context_add(context, |parser| {
            loop {
                if parser.at(close)
                    || matches!(parser.cur_kind(), Kind::Eof | Kind::Undetermined)
                    || parser.has_fatal_error()
                {
                    break;
                }
                if let Some(element) = parse_element(parser) {
                    list.push(element);
                } else {
                    break;
                }
            }
        });
        self.expect_recoverable_closing(close, opening_span, context);
        list
    }

    pub(crate) fn parse_delimited_list<F, T>(
        &mut self,
        close: Kind,
        separator: Kind,
        opening_span: Span,
        parse_element: F,
    ) -> (ArenaVec<'a, T>, Option<u32>)
    where
        F: FnMut(&mut Self) -> T,
    {
        let mut list = ArenaVec::new_in(self);
        let trailing_separator = self.parse_delimited_list_into(
            &mut list,
            close,
            separator,
            opening_span,
            parse_element,
        );
        (list, trailing_separator)
    }

    #[expect(clippy::inline_always)]
    #[inline(always)]
    pub(crate) fn parse_recoverable_delimited_list<F, T>(
        &mut self,
        context: RecoveryContext,
        close: Kind,
        separator: Kind,
        opening_span: Span,
        parse_element: F,
    ) -> (ArenaVec<'a, T>, Option<u32>)
    where
        F: FnMut(&mut Self) -> T,
    {
        self.recovery_context_add(context, |parser| {
            let mut list = ArenaVec::new_in(&*parser);
            let trailing_separator = parser.parse_delimited_list_into_impl(
                &mut list,
                close,
                separator,
                opening_span,
                parse_element,
                Some(context),
            );
            (list, trailing_separator)
        })
    }

    #[expect(clippy::inline_always)]
    #[inline(always)]
    pub(crate) fn parse_delimited_list_into<F, T>(
        &mut self,
        list: &mut ArenaVec<'a, T>,
        close: Kind,
        separator: Kind,
        opening_span: Span,
        parse_element: F,
    ) -> Option<u32>
    where
        F: FnMut(&mut Self) -> T,
    {
        self.parse_delimited_list_into_impl(
            list,
            close,
            separator,
            opening_span,
            parse_element,
            None,
        )
    }

    #[expect(clippy::inline_always)]
    #[inline(always)]
    fn parse_delimited_list_into_impl<F, T>(
        &mut self,
        list: &mut ArenaVec<'a, T>,
        close: Kind,
        separator: Kind,
        opening_span: Span,
        mut parse_element: F,
        recovery_context: Option<RecoveryContext>,
    ) -> Option<u32>
    where
        F: FnMut(&mut Self) -> T,
    {
        // Cache cur_kind() to avoid redundant calls in compound checks
        let kind = self.cur_kind();
        if kind == close
            || matches!(kind, Kind::Eof | Kind::Undetermined)
            || self.fatal_error.is_some()
        {
            return None;
        }
        if self.options.mode == ParseMode::Editor
            && recovery_context == Some(RecoveryContext::TypeArguments)
            && self.at_recovery_type_argument_closing_boundary()
        {
            return None;
        }
        if unlikely(
            self.options.mode == ParseMode::Editor
                && recovery_context
                    .is_some_and(|context| self.at_recovery_list_outer_boundary(context)),
        ) {
            let context = recovery_context.expect("checked as present");
            let span = Span::empty(self.cur_start());
            self.record_missing_token_recovery(close, span);
            let diagnostic = match context {
                RecoveryContext::ObjectProperties => {
                    diagnostics::property_assignment_expected(span)
                }
                RecoveryContext::ArrayElements => diagnostics::expression_or_comma_expected(span),
                RecoveryContext::Arguments => diagnostics::argument_expression_expected(span),
                _ => unreachable!("only expression list contexts use delimiter recovery"),
            };
            self.error(diagnostic);
            return None;
        }
        let element = parse_element(self);
        list.push(element);
        loop {
            let kind = self.cur_kind();
            if kind == close
                || matches!(kind, Kind::Eof | Kind::Undetermined)
                || self.fatal_error.is_some()
            {
                return None;
            }
            if kind != separator {
                if self.options.mode == ParseMode::Editor
                    && recovery_context == Some(RecoveryContext::TypeArguments)
                    && self.at_recovery_type_argument_closing_boundary()
                {
                    return None;
                }
                if unlikely(
                    self.options.mode == ParseMode::Editor
                        && recovery_context
                            .is_some_and(|context| self.at_recovery_list_outer_boundary(context)),
                ) {
                    let span = Span::empty(self.cur_start());
                    self.record_missing_token_recovery(close, span);
                    self.error(diagnostics::typescript_expected_token(separator.to_str(), span));
                    return None;
                }
                if unlikely(self.options.mode == ParseMode::Editor)
                    && recovery_context
                        .is_some_and(|context| self.is_recovery_list_element_start(context))
                {
                    let span = Span::empty(self.cur_start());
                    self.record_missing_token_recovery(separator, span);
                    self.error(diagnostics::typescript_expected_token(separator.to_str(), span));
                    let element_start = self.cur_start();
                    let element = parse_element(self);
                    list.push(element);
                    debug_assert!(
                        self.cur_start() != element_start || self.fatal_error.is_some(),
                        "a recovered missing separator must still parse a progressing element"
                    );
                    continue;
                }
                self.set_fatal_error(diagnostics::expect_closing_or_separator(
                    close.to_str(),
                    separator.to_str(),
                    kind.to_str(),
                    self.cur_token().span(),
                    opening_span,
                ));
                return None;
            }
            self.advance(separator);
            let kind = self.cur_kind();
            if kind == close {
                let trailing_separator = self.prev_token_end - 1;
                return Some(trailing_separator);
            }
            if matches!(kind, Kind::Eof | Kind::Undetermined) {
                return None;
            }
            let element = parse_element(self);
            list.push(element);
        }
    }

    fn is_recovery_list_element_start(&mut self, context: RecoveryContext) -> bool {
        let kind = self.cur_kind();
        if context == RecoveryContext::ObjectProperties {
            return matches!(kind, Kind::LBrack | Kind::Star | Kind::Dot3)
                || kind.is_literal_property_name();
        }
        if context.intersects(RecoveryContext::ArrayElements | RecoveryContext::Arguments) {
            return matches!(kind, Kind::Comma | Kind::Dot3) || self.is_start_of_expression();
        }
        false
    }

    fn at_recovery_type_argument_closing_boundary(&self) -> bool {
        matches!(
            self.cur_kind(),
            Kind::Eq
                | Kind::Semicolon
                | Kind::RCurly
                | Kind::RBrack
                | Kind::RParen
                | Kind::Eof
                | Kind::Undetermined
        )
    }

    pub(crate) fn at_recovery_list_outer_boundary(&self, context: RecoveryContext) -> bool {
        if context == RecoveryContext::Arguments
            && self.cur_token().is_on_new_line()
            && self
                .recovery_ctx
                .intersects(RecoveryContext::SourceElements | RecoveryContext::BlockStatements)
            && matches!(self.cur_kind(), Kind::Const | Kind::Var)
        {
            return true;
        }

        match self.cur_kind() {
            Kind::Eof | Kind::Undetermined => true,
            Kind::Eq | Kind::Semicolon => context.intersects(
                RecoveryContext::TypeArguments
                    | RecoveryContext::ParenthesizedType
                    | RecoveryContext::ArrayTypeSuffix,
            ),
            Kind::RCurly => context != RecoveryContext::ObjectProperties,
            Kind::RBrack => {
                context != RecoveryContext::ArrayElements
                    && context != RecoveryContext::ArrayTypeSuffix
            }
            Kind::RParen => {
                context != RecoveryContext::Arguments
                    && context != RecoveryContext::ParenthesizedType
                    && context != RecoveryContext::Parameters
            }
            Kind::RAngle => context != RecoveryContext::TypeArguments,
            _ => false,
        }
    }

    #[expect(clippy::inline_always)]
    #[inline(always)]
    pub(crate) fn expect_recoverable_closing(
        &mut self,
        close: Kind,
        opening_span: Span,
        context: RecoveryContext,
    ) {
        if self.at(close) {
            self.advance(close);
        } else if unlikely(self.options.mode == ParseMode::Editor)
            && self.at_recovery_list_outer_boundary(context)
        {
            let span = Span::empty(self.cur_start());
            if !self.last_recovery_is_missing_token(close, span) {
                self.record_missing_token_recovery(close, span);
                let expected = if context == RecoveryContext::Parameters
                    && !matches!(self.cur_kind(), Kind::Eof | Kind::Undetermined)
                {
                    Kind::Comma
                } else {
                    close
                };
                self.error(diagnostics::typescript_expected_token(expected.to_str(), span));
            }
        } else {
            self.expect_closing(close, opening_span);
        }
    }

    pub(crate) fn parse_delimited_list_with_rest<E, A, R, D>(
        &mut self,
        close: Kind,
        opening_span: Span,
        parse_element: E,
        parse_rest: R,
        rest_last_diagnostic: D,
    ) -> (ArenaVec<'a, A>, Option<ArenaBox<'a, BindingRestElement<'a>>>)
    where
        E: Fn(&mut Self) -> A,
        R: Fn(&mut Self) -> ArenaBox<'a, BindingRestElement<'a>>,
        D: Fn(Span) -> ParserDiagnostic<'a>,
    {
        let mut list = ArenaVec::new_in(self);
        let rest = self.parse_delimited_list_with_rest_into(
            &mut list,
            close,
            opening_span,
            parse_element,
            parse_rest,
            rest_last_diagnostic,
        );
        (list, rest)
    }

    #[expect(clippy::inline_always)]
    #[inline(always)]
    fn parse_delimited_list_with_rest_into<E, A, R, D>(
        &mut self,
        list: &mut ArenaVec<'a, A>,
        close: Kind,
        opening_span: Span,
        parse_element: E,
        parse_rest: R,
        rest_last_diagnostic: D,
    ) -> Option<ArenaBox<'a, BindingRestElement<'a>>>
    where
        E: Fn(&mut Self) -> A,
        R: Fn(&mut Self) -> ArenaBox<'a, BindingRestElement<'a>>,
        D: Fn(Span) -> ParserDiagnostic<'a>,
    {
        let mut rest: Option<ArenaBox<'a, BindingRestElement<'a>>> = None;
        let mut first = true;
        loop {
            let kind = self.cur_kind();
            if kind == close
                || matches!(kind, Kind::Eof | Kind::Undetermined)
                || self.fatal_error.is_some()
            {
                break;
            }

            if first {
                first = false;
            } else {
                let comma_span = self.cur_token().span();
                if kind != Kind::Comma {
                    let error = diagnostics::expect_closing_or_separator(
                        close.to_str(),
                        Kind::Comma.to_str(),
                        kind.to_str(),
                        comma_span,
                        opening_span,
                    );
                    self.set_fatal_error(error);
                    break;
                }
                self.bump_any();
                let kind = self.cur_kind();
                if kind == close {
                    if rest.is_some() && !self.ctx.has_ambient() {
                        self.error(diagnostics::rest_element_trailing_comma(comma_span));
                    }
                    break;
                }
                if matches!(kind, Kind::Eof | Kind::Undetermined) {
                    break;
                }
            }

            if let Some(r) = &rest {
                self.set_fatal_error(rest_last_diagnostic(r.span()));
                break;
            }

            // Re-capture kind to get the current token (may have changed after else branch)
            let kind = self.cur_kind();
            if kind == Kind::Dot3 {
                rest.replace(parse_rest(self));
            } else {
                let element = parse_element(self);
                list.push(element);
            }
        }

        rest
    }
}
