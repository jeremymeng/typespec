#![doc = "Scanner crate for the TypeSpec Rust port."]

pub mod charcode;
pub mod token;

use std::cmp::min;

use charcode::*;
use string_interner::DefaultStringInterner;
use token::{keyword, token_display, Token, KEYWORD_MAX_LENGTH, KEYWORD_MIN_LENGTH};
use typespec_source::{Diagnostic, DiagnosticSeverity, SourceFile, Span};
use unicode_normalization::UnicodeNormalization;

const MERGE_CONFLICT_MARKER_LENGTH: u32 = 7;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TokenFlags(u32);

impl TokenFlags {
    pub const NONE: Self = Self(0);
    pub const ESCAPED: Self = Self(1 << 0);
    pub const TRIPLE_QUOTED: Self = Self(1 << 1);
    pub const UNTERMINATED: Self = Self(1 << 2);
    pub const NON_ASCII: Self = Self(1 << 3);
    pub const DOC_COMMENT: Self = Self(1 << 4);
    pub const BACKTICKED: Self = Self(1 << 5);

    pub fn contains(self, other: Self) -> bool {
        self.0 & other.0 != 0
    }

    fn insert(&mut self, other: Self) {
        self.0 |= other.0;
    }
}

pub struct Scanner<'src> {
    units: Vec<u16>,
    position: u32,
    end_position: u32,
    token: Token,
    token_position: u32,
    token_flags: TokenFlags,
    on_diagnostic: Box<dyn FnMut(Diagnostic) + 'src>,
    interner: DefaultStringInterner,
}

impl<'src> Scanner<'src> {
    pub fn new<F>(source: &'src SourceFile, on_diagnostic: F) -> Self
    where
        F: FnMut(Diagnostic) + 'src,
    {
        let units: Vec<u16> = source.text().encode_utf16().collect();
        let mut scanner = Self {
            end_position: units.len() as u32,
            units,
            position: 0,
            token: Token::None,
            token_position: 0,
            token_flags: TokenFlags::NONE,
            on_diagnostic: Box::new(on_diagnostic),
            interner: DefaultStringInterner::new(),
        };
        if scanner.char_at(0) == Some(BYTE_ORDER_MARK) {
            scanner.position = 1;
        }
        scanner
    }

    pub fn scan(&mut self) -> Token {
        self.token_position = self.position;
        self.token_flags = TokenFlags::NONE;

        if self.eof() {
            self.token = Token::EndOfFile;
            return self.token;
        }

        let ch = self.char_at(self.position).unwrap();
        match ch {
            CARRIAGE_RETURN => {
                if self.look_ahead(1) == Some(LINE_FEED) {
                    self.position += 1;
                }
                self.next(Token::NewLine, 1)
            }
            LINE_FEED => self.next(Token::NewLine, 1),
            SPACE | TAB | VERTICAL_TAB | FORM_FEED => self.scan_whitespace(),
            0x28 => self.next(Token::OpenParen, 1),
            0x29 => self.next(Token::CloseParen, 1),
            0x2c => self.next(Token::Comma, 1),
            0x3a => {
                if self.look_ahead(1) == Some(0x3a) {
                    self.next(Token::ColonColon, 2)
                } else {
                    self.next(Token::Colon, 1)
                }
            }
            0x3b => self.next(Token::Semicolon, 1),
            0x5b => self.next(Token::OpenBracket, 1),
            0x5d => self.next(Token::CloseBracket, 1),
            0x7b => self.next(Token::OpenBrace, 1),
            0x7d => self.next(Token::CloseBrace, 1),
            0x40 => {
                if self.look_ahead(1) == Some(0x40) {
                    self.next(Token::AtAt, 2)
                } else {
                    self.next(Token::At, 1)
                }
            }
            0x23 => match self.look_ahead(1) {
                Some(0x7b) => self.next(Token::HashBrace, 2),
                Some(0x5b) => self.next(Token::HashBracket, 2),
                _ => self.next(Token::Hash, 1),
            },
            0x2b => {
                if self.look_ahead(1).is_some_and(is_digit) {
                    self.scan_signed_number()
                } else {
                    self.next(Token::Plus, 1)
                }
            }
            0x2d => {
                if self.look_ahead(1).is_some_and(is_digit) {
                    self.scan_signed_number()
                } else {
                    self.next(Token::Hyphen, 1)
                }
            }
            0x2a => self.next(Token::Star, 1),
            0x3f => self.next(Token::Question, 1),
            0x26 => {
                if self.look_ahead(1) == Some(0x26) {
                    self.next(Token::AmpsersandAmpersand, 2)
                } else {
                    self.next(Token::Ampersand, 1)
                }
            }
            0x2e => {
                if self.look_ahead(1) == Some(0x2e) && self.look_ahead(2) == Some(0x2e) {
                    self.next(Token::Ellipsis, 3)
                } else {
                    self.next(Token::Dot, 1)
                }
            }
            0x2f => match self.look_ahead(1) {
                Some(0x2f) => self.scan_single_line_comment(),
                Some(0x2a) => self.scan_multi_line_comment(),
                _ => self.next(Token::ForwardSlash, 1),
            },
            0x30 => match self.look_ahead(1) {
                Some(0x78) => self.scan_hex_number(),
                Some(0x62) => self.scan_binary_number(),
                _ => self.scan_number(),
            },
            0x31..=0x39 => self.scan_number(),
            0x3c => {
                if self.at_conflict_marker() {
                    self.scan_conflict_marker()
                } else if self.look_ahead(1) == Some(0x3d) {
                    self.next(Token::LessThanEquals, 2)
                } else {
                    self.next(Token::LessThan, 1)
                }
            }
            0x3e => {
                if self.at_conflict_marker() {
                    self.scan_conflict_marker()
                } else if self.look_ahead(1) == Some(0x3d) {
                    self.next(Token::GreaterThanEquals, 2)
                } else {
                    self.next(Token::GreaterThan, 1)
                }
            }
            0x3d => {
                if self.at_conflict_marker() {
                    self.scan_conflict_marker()
                } else {
                    match self.look_ahead(1) {
                        Some(0x3d) => self.next(Token::EqualsEquals, 2),
                        Some(0x3e) => self.next(Token::EqualsGreaterThan, 2),
                        _ => self.next(Token::Equals, 1),
                    }
                }
            }
            0x7c => {
                if self.at_conflict_marker() {
                    self.scan_conflict_marker()
                } else if self.look_ahead(1) == Some(0x7c) {
                    self.next(Token::BarBar, 2)
                } else {
                    self.next(Token::Bar, 1)
                }
            }
            0x22 => {
                if self.look_ahead(1) == Some(0x22) && self.look_ahead(2) == Some(0x22) {
                    self.scan_string(TokenFlags::TRIPLE_QUOTED)
                } else {
                    self.scan_string(TokenFlags::NONE)
                }
            }
            0x21 => {
                if self.look_ahead(1) == Some(0x3d) {
                    self.next(Token::ExclamationEquals, 2)
                } else {
                    self.next(Token::Exclamation, 1)
                }
            }
            0x60 => self.scan_backticked_identifier(),
            _ if is_lowercase_ascii_letter(ch) => self.scan_identifier_or_keyword(),
            _ if is_ascii_identifier_start(ch) => self.scan_identifier(),
            _ if ch <= MAX_ASCII => self.scan_invalid_character(),
            _ => self.scan_non_ascii_token(),
        }
    }

    pub fn scan_doc(&mut self) -> Token {
        self.token_position = self.position;
        self.token_flags = TokenFlags::NONE;
        if self.eof() {
            self.token = Token::EndOfFile;
            return self.token;
        }

        let ch = self.char_at(self.position).unwrap();
        match ch {
            CARRIAGE_RETURN => {
                if self.look_ahead(1) == Some(LINE_FEED) {
                    self.position += 1;
                }
                self.next(Token::NewLine, 1)
            }
            LINE_FEED => self.next(Token::NewLine, 1),
            0x5c => {
                if self.look_ahead(1) == Some(0x40) {
                    self.token_flags.insert(TokenFlags::ESCAPED);
                    self.next(Token::DocText, 2)
                } else {
                    self.next(Token::DocText, 1)
                }
            }
            SPACE | TAB | VERTICAL_TAB | FORM_FEED => self.scan_whitespace(),
            0x7d => self.next(Token::CloseBrace, 1),
            0x40 => self.next(Token::At, 1),
            0x2a => self.next(Token::Star, 1),
            0x60 => {
                if self.look_ahead(1) == Some(0x60) && self.look_ahead(2) == Some(0x60) {
                    self.next(Token::DocCodeFenceDelimiter, 3)
                } else {
                    self.scan_doc_code_span()
                }
            }
            0x3c | 0x3e | 0x3d | 0x7c => {
                if self.at_conflict_marker() {
                    self.scan_conflict_marker()
                } else {
                    self.next(Token::DocText, 1)
                }
            }
            0x2d => self.next(Token::Hyphen, 1),
            _ if is_ascii_identifier_start(ch) => self.scan_identifier(),
            _ if ch <= MAX_ASCII => self.next(Token::DocText, 1),
            _ => {
                let cp = self.code_point_at(self.position).unwrap();
                if is_identifier_start(cp) {
                    self.scan_non_ascii_identifier(cp)
                } else {
                    self.scan_unknown(Token::DocText)
                }
            }
        }
    }

    pub fn rescan_string_template(&mut self, last_token_flags: TokenFlags) -> Token {
        self.position = self.token_position;
        self.token_flags = TokenFlags::NONE;
        self.scan_string_template_span(last_token_flags)
    }

    pub fn eof(&self) -> bool {
        self.position >= self.end_position
    }

    pub fn position(&self) -> u32 {
        self.position
    }

    pub fn token(&self) -> Token {
        self.token
    }

    pub fn token_position(&self) -> u32 {
        self.token_position
    }

    pub fn token_flags(&self) -> TokenFlags {
        self.token_flags
    }

    pub fn token_text(&self) -> String {
        self.slice_text(self.token_position, self.position)
    }

    pub fn token_value(&mut self) -> Option<String> {
        match self.token {
            Token::StringLiteral
            | Token::StringTemplateHead
            | Token::StringTemplateMiddle
            | Token::StringTemplateTail => Some(self.get_string_token_value()),
            Token::Identifier => {
                let value = self.get_identifier_token_value();
                self.interner.get_or_intern(&value);
                Some(value)
            }
            Token::NumericLiteral => Some(self.token_text()),
            Token::DocText => Some(self.get_doc_text_value()),
            _ => None,
        }
    }

    fn next(&mut self, token: Token, count: u32) -> Token {
        self.position += count;
        self.token = token;
        token
    }

    fn unterminated(&mut self, token: Token) -> Token {
        self.token_flags.insert(TokenFlags::UNTERMINATED);
        self.error(
            "unterminated",
            &format!("Unterminated {}.", token_display(token)),
            None,
            None,
        );
        self.token = token;
        token
    }

    fn error(&mut self, code: &'static str, message: &str, pos: Option<u32>, end: Option<u32>) {
        (self.on_diagnostic)(Diagnostic {
            severity: DiagnosticSeverity::Error,
            code,
            message: message.to_string(),
            target: Span {
                pos: pos.unwrap_or(self.token_position),
                end: end.unwrap_or(self.position),
            },
        });
    }

    fn char_at(&self, pos: u32) -> Option<u32> {
        self.units.get(pos as usize).map(|x| *x as u32)
    }

    fn look_ahead(&self, offset: u32) -> Option<u32> {
        self.char_at(self.position + offset)
    }

    fn code_point_at(&self, pos: u32) -> Option<u32> {
        let ch = *self.units.get(pos as usize)?;
        if charcode::is_high_surrogate(ch) {
            if let Some(next) = self.units.get(pos as usize + 1) {
                if charcode::is_low_surrogate(*next) {
                    let high = (ch as u32) - 0xd800;
                    let low = (*next as u32) - 0xdc00;
                    return Some((high << 10) + low + 0x10000);
                }
            }
        }
        Some(ch as u32)
    }

    fn slice_text(&self, start: u32, end: u32) -> String {
        let start = start as usize;
        let end = min(end as usize, self.units.len());
        String::from_utf16_lossy(&self.units[start..end])
    }

    fn scan_non_ascii_token(&mut self) -> Token {
        self.token_flags.insert(TokenFlags::NON_ASCII);
        let ch = self.char_at(self.position).unwrap();
        if is_non_ascii_whitespace_single_line(ch) {
            return self.scan_whitespace();
        }
        let cp = self.code_point_at(self.position).unwrap();
        if is_non_ascii_identifier_character(cp) {
            return self.scan_non_ascii_identifier(cp);
        }
        self.scan_invalid_character()
    }

    fn scan_invalid_character(&mut self) -> Token {
        let token = self.scan_unknown(Token::Invalid);
        self.error("invalid-character", "Invalid character.", None, None);
        token
    }

    fn scan_unknown(&mut self, token: Token) -> Token {
        let cp = self.code_point_at(self.position).unwrap();
        self.next(token, utf16_code_units(cp))
    }

    fn scan_whitespace(&mut self) -> Token {
        loop {
            self.position += 1;
            if self.eof() || !is_whitespace_single_line(self.char_at(self.position).unwrap()) {
                break;
            }
        }
        self.token = Token::Whitespace;
        Token::Whitespace
    }

    fn scan_signed_number(&mut self) -> Token {
        self.position += 1;
        self.scan_number()
    }

    fn scan_number(&mut self) -> Token {
        self.scan_known_digits();
        if !self.eof() && self.char_at(self.position) == Some(0x2e) {
            self.position += 1;
            self.scan_required_digits();
        }
        if !self.eof() && self.char_at(self.position) == Some(0x65) {
            self.position += 1;
            if matches!(self.char_at(self.position), Some(0x2b | 0x2d)) {
                self.position += 1;
            }
            self.scan_required_digits();
        }
        self.token = Token::NumericLiteral;
        Token::NumericLiteral
    }

    fn scan_known_digits(&mut self) {
        loop {
            self.position += 1;
            if self.eof() || !is_digit(self.char_at(self.position).unwrap()) {
                break;
            }
        }
    }

    fn scan_required_digits(&mut self) {
        if self.eof() || !is_digit(self.char_at(self.position).unwrap()) {
            self.error("digit-expected", "Digit expected.", None, None);
            return;
        }
        self.scan_known_digits();
    }

    fn scan_hex_number(&mut self) -> Token {
        self.position += 2;
        if self.eof() || !is_hex_digit(self.char_at(self.position).unwrap()) {
            self.error(
                "hex-digit-expected",
                "Hexadecimal digit expected.",
                None,
                None,
            );
            self.token = Token::NumericLiteral;
            return Token::NumericLiteral;
        }
        loop {
            self.position += 1;
            if self.eof() || !is_hex_digit(self.char_at(self.position).unwrap()) {
                break;
            }
        }
        self.token = Token::NumericLiteral;
        Token::NumericLiteral
    }

    fn scan_binary_number(&mut self) -> Token {
        self.position += 2;
        if self.eof() || !is_binary_digit(self.char_at(self.position).unwrap()) {
            self.error(
                "binary-digit-expected",
                "Binary digit expected.",
                None,
                None,
            );
            self.token = Token::NumericLiteral;
            return Token::NumericLiteral;
        }
        loop {
            self.position += 1;
            if self.eof() || !is_binary_digit(self.char_at(self.position).unwrap()) {
                break;
            }
        }
        self.token = Token::NumericLiteral;
        Token::NumericLiteral
    }

    fn scan_single_line_comment(&mut self) -> Token {
        self.position += 2;
        while !self.eof() && !is_line_break(self.char_at(self.position).unwrap()) {
            self.position += 1;
        }
        self.token = Token::SingleLineComment;
        Token::SingleLineComment
    }

    fn scan_multi_line_comment(&mut self) -> Token {
        self.token = Token::MultiLineComment;
        if self.look_ahead(2) == Some(0x2a) {
            self.token_flags.insert(TokenFlags::DOC_COMMENT);
        }
        self.position += 2;
        while !self.eof() {
            if self.char_at(self.position) == Some(0x2a)
                && self.char_at(self.position + 1) == Some(0x2f)
            {
                self.position += 2;
                return Token::MultiLineComment;
            }
            self.position += 1;
        }
        self.unterminated(Token::MultiLineComment)
    }

    fn scan_doc_code_span(&mut self) -> Token {
        self.position += 1;
        while !self.eof() {
            match self.char_at(self.position).unwrap() {
                0x60 => {
                    self.position += 1;
                    self.token = Token::DocCodeSpan;
                    return Token::DocCodeSpan;
                }
                CARRIAGE_RETURN | LINE_FEED => break,
                _ => self.position += 1,
            }
        }
        self.unterminated(Token::DocCodeSpan)
    }

    fn scan_string(&mut self, flags: TokenFlags) -> Token {
        if flags.contains(TokenFlags::TRIPLE_QUOTED) {
            self.position += 3;
        } else {
            self.position += 1;
        }
        self.scan_string_literal_like(flags, Token::StringTemplateHead, Token::StringLiteral)
    }

    fn scan_string_template_span(&mut self, flags: TokenFlags) -> Token {
        self.position += 1;
        self.scan_string_literal_like(
            flags,
            Token::StringTemplateMiddle,
            Token::StringTemplateTail,
        )
    }

    fn scan_string_literal_like(
        &mut self,
        requested_flags: TokenFlags,
        template: Token,
        tail: Token,
    ) -> Token {
        let multi_line = requested_flags.contains(TokenFlags::TRIPLE_QUOTED);
        self.token_flags = requested_flags;
        while !self.eof() {
            match self.char_at(self.position).unwrap() {
                0x5c => {
                    self.token_flags.insert(TokenFlags::ESCAPED);
                    self.position += 1;
                    if self.eof() {
                        break;
                    }
                    self.position += 1;
                    continue;
                }
                0x22 => {
                    if multi_line {
                        if self.look_ahead(1) == Some(0x22) && self.look_ahead(2) == Some(0x22) {
                            self.position += 3;
                            self.token = tail;
                            return tail;
                        }
                    } else {
                        self.position += 1;
                        self.token = tail;
                        return tail;
                    }
                }
                0x24 => {
                    if self.look_ahead(1) == Some(0x7b) {
                        self.position += 2;
                        self.token = template;
                        return template;
                    }
                }
                CARRIAGE_RETURN | LINE_FEED if !multi_line => break,
                CARRIAGE_RETURN | LINE_FEED => {}
                _ => {}
            }
            self.position += 1;
        }
        self.unterminated(tail)
    }

    fn string_literal_offset_start(&self, token: Token, flags: TokenFlags) -> u32 {
        match token {
            Token::StringLiteral | Token::StringTemplateHead
                if flags.contains(TokenFlags::TRIPLE_QUOTED) =>
            {
                3
            }
            Token::StringLiteral | Token::StringTemplateHead => 1,
            _ => 1,
        }
    }

    fn string_literal_offset_end(&self, token: Token, flags: TokenFlags) -> u32 {
        match token {
            Token::StringLiteral | Token::StringTemplateTail => {
                if flags.contains(TokenFlags::TRIPLE_QUOTED) {
                    3
                } else {
                    1
                }
            }
            _ => 2,
        }
    }

    fn get_string_token_value(&mut self) -> String {
        if self.token_flags.contains(TokenFlags::TRIPLE_QUOTED) {
            let start = self.token_position;
            let end = self.position;
            let (indentation_start, indentation_end) =
                self.find_triple_quoted_string_indent(start, end);
            return self.unindent_and_unescape_triple_quoted_string(
                start,
                end,
                indentation_start,
                indentation_end,
                self.token,
                self.token_flags,
            );
        }

        let start_offset = self.string_literal_offset_start(self.token, self.token_flags);
        let end_offset = self.string_literal_offset_end(self.token, self.token_flags);
        let start = self.token_position + start_offset;
        let end = if self.token_flags.contains(TokenFlags::UNTERMINATED) {
            self.position
        } else {
            self.position.saturating_sub(end_offset)
        };
        if self.token_flags.contains(TokenFlags::ESCAPED) {
            self.unescape_string(start, end)
        } else {
            self.slice_text(start, end)
        }
    }

    fn get_identifier_token_value(&mut self) -> String {
        let start = if self.token_flags.contains(TokenFlags::BACKTICKED) {
            self.token_position + 1
        } else {
            self.token_position
        };
        let end = if self.token_flags.contains(TokenFlags::BACKTICKED)
            && !self.token_flags.contains(TokenFlags::UNTERMINATED)
        {
            self.position.saturating_sub(1)
        } else {
            self.position
        };
        let text = if self.token_flags.contains(TokenFlags::ESCAPED) {
            self.unescape_string(start, end)
        } else {
            self.slice_text(start, end)
        };
        if self.token_flags.contains(TokenFlags::NON_ASCII) {
            text.nfc().collect()
        } else {
            text
        }
    }

    fn get_doc_text_value(&self) -> String {
        if !self.token_flags.contains(TokenFlags::ESCAPED) {
            return self.slice_text(self.token_position, self.position);
        }
        let mut start = self.token_position;
        let end = self.position;
        let mut result = String::new();
        let mut pos = start;
        while pos < end {
            if self.char_at(pos) != Some(0x5c) {
                pos += 1;
                continue;
            }
            if pos == end - 1 {
                break;
            }
            result.push_str(&self.slice_text(start, pos));
            if self.char_at(pos + 1) == Some(0x40) {
                result.push('@');
            } else {
                result.push_str(&self.slice_text(pos, pos + 2));
            }
            pos += 2;
            start = pos;
        }
        result.push_str(&self.slice_text(start, end));
        result
    }

    pub fn find_triple_quoted_string_indent(&self, start: u32, mut end: u32) -> (u32, u32) {
        end = end.saturating_sub(3);
        let indentation_end = end;
        while end > start && self.char_at(end - 1).is_some_and(is_whitespace_single_line) {
            end -= 1;
        }
        let indentation_start = end;
        if end > start && self.char_at(end - 1).is_some_and(is_line_break) {
            if self.is_crlf(end.saturating_sub(2), 0, end) {
                end -= 1;
            }
            let _ = end.saturating_sub(1);
        }
        (indentation_start, indentation_end)
    }

    #[allow(clippy::too_many_arguments)]
    pub fn unindent_and_unescape_triple_quoted_string(
        &mut self,
        mut start: u32,
        mut end: u32,
        indentation_start: u32,
        indentation_end: u32,
        token: Token,
        flags: TokenFlags,
    ) -> String {
        let start_offset = self.string_literal_offset_start(token, flags);
        let end_offset = self.string_literal_offset_end(token, flags);
        start += start_offset;
        if !flags.contains(TokenFlags::UNTERMINATED) {
            end = end.saturating_sub(end_offset);
        }

        if matches!(token, Token::StringLiteral | Token::StringTemplateHead) {
            while start < end && self.char_at(start).is_some_and(is_whitespace_single_line) {
                start += 1;
            }
            if start < end && self.char_at(start).is_some_and(is_line_break) {
                if self.is_crlf(start, start, end) {
                    start += 1;
                }
                start += 1;
            } else {
                self.error(
                    "no-new-line-start-triple-quote",
                    "String content in triple quotes must begin on a new line.",
                    None,
                    None,
                );
            }
        }

        if matches!(token, Token::StringLiteral | Token::StringTemplateTail) {
            while end > start && self.char_at(end - 1).is_some_and(is_whitespace_single_line) {
                end -= 1;
            }
            if end > start && self.char_at(end - 1).is_some_and(is_line_break) {
                if self.is_crlf(end.saturating_sub(2), start, end) {
                    end -= 1;
                }
                end -= 1;
            } else {
                self.error(
                    "no-new-line-end-triple-quote",
                    "Closing triple quotes must begin on a new line.",
                    None,
                    None,
                );
            }
        }

        let mut skip_unindent_once = matches!(
            token,
            Token::StringTemplateMiddle | Token::StringTemplateTail
        );
        let mut result = String::new();
        let mut pos = start;
        while pos < end {
            if skip_unindent_once {
                skip_unindent_once = false;
            } else {
                start =
                    self.skip_matching_indentation(pos, end, indentation_start, indentation_end);
                pos = start;
            }
            while pos < end && !self.char_at(pos).is_some_and(is_line_break) {
                if self.char_at(pos) != Some(0x5c) {
                    pos += 1;
                    continue;
                }
                result.push_str(&self.slice_text(start, pos));
                if pos == end - 1 {
                    self.error(
                        "invalid-escape-sequence",
                        "Invalid escape sequence.",
                        Some(pos),
                        Some(pos),
                    );
                    pos += 1;
                } else {
                    result.push_str(&self.unescape_one(pos));
                    pos += 2;
                }
                start = pos;
            }
            if pos < end {
                if self.is_crlf(pos, start, end) {
                    result.push_str(&self.slice_text(start, pos));
                    result.push('\n');
                    pos += 2;
                } else {
                    pos += 1;
                    result.push_str(&self.slice_text(start, pos));
                }
                start = pos;
            }
        }
        result.push_str(&self.slice_text(start, pos));
        result
    }

    fn is_crlf(&self, pos: u32, start: u32, end: u32) -> bool {
        pos >= start
            && pos < end.saturating_sub(1)
            && self.char_at(pos) == Some(CARRIAGE_RETURN)
            && self.char_at(pos + 1) == Some(LINE_FEED)
    }

    fn skip_matching_indentation(
        &mut self,
        mut pos: u32,
        end: u32,
        indentation_start: u32,
        indentation_end: u32,
    ) -> u32 {
        let mut indentation_pos = indentation_start;
        let stop = min(end, pos + (indentation_end - indentation_start));
        while pos < stop {
            let ch = self.char_at(pos).unwrap();
            if is_line_break(ch) {
                break;
            }
            if Some(ch) != self.char_at(indentation_pos) {
                self.error(
                    "triple-quote-indent",
                    "All lines in triple-quoted string lines must have the same indentation as closing triple quotes.",
                    None,
                    None,
                );
                break;
            }
            indentation_pos += 1;
            pos += 1;
        }
        pos
    }

    fn unescape_string(&mut self, mut start: u32, end: u32) -> String {
        let mut result = String::new();
        let mut pos = start;
        while pos < end {
            if self.char_at(pos) != Some(0x5c) {
                pos += 1;
                continue;
            }
            if pos == end - 1 {
                self.error(
                    "invalid-escape-sequence",
                    "Invalid escape sequence.",
                    Some(pos),
                    Some(pos),
                );
                break;
            }
            result.push_str(&self.slice_text(start, pos));
            result.push_str(&self.unescape_one(pos));
            pos += 2;
            start = pos;
        }
        result.push_str(&self.slice_text(start, pos));
        result
    }

    fn unescape_one(&mut self, pos: u32) -> String {
        match self.char_at(pos + 1).unwrap_or(0) {
            0x72 => "\r".to_string(),
            0x6e => "\n".to_string(),
            0x74 => "\t".to_string(),
            0x22 => "\"".to_string(),
            0x5c => "\\".to_string(),
            0x24 => "$".to_string(),
            0x40 => "@".to_string(),
            0x60 => "`".to_string(),
            ch => {
                self.error(
                    "invalid-escape-sequence",
                    "Invalid escape sequence.",
                    Some(pos),
                    Some(pos + 2),
                );
                String::from_utf16_lossy(&[ch as u16])
            }
        }
    }

    fn scan_identifier_or_keyword(&mut self) -> Token {
        let mut count = 0usize;
        loop {
            self.position += 1;
            count += 1;
            if self.eof() {
                break;
            }
            let ch = self.char_at(self.position).unwrap();
            if count < KEYWORD_MAX_LENGTH && is_lowercase_ascii_letter(ch) {
                continue;
            }
            if is_ascii_identifier_continue(ch) {
                return self.scan_identifier();
            }
            if ch > MAX_ASCII {
                let cp = self.code_point_at(self.position).unwrap();
                if is_non_ascii_identifier_character(cp) {
                    return self.scan_non_ascii_identifier(cp);
                }
            }
            break;
        }

        if (KEYWORD_MIN_LENGTH..=KEYWORD_MAX_LENGTH).contains(&count) {
            if let Some(keyword) = keyword(&self.token_text()) {
                self.token = keyword;
                return keyword;
            }
        }
        self.token = Token::Identifier;
        Token::Identifier
    }

    fn scan_identifier(&mut self) -> Token {
        loop {
            self.position += 1;
            if self.eof() {
                self.token = Token::Identifier;
                return Token::Identifier;
            }
            let ch = self.char_at(self.position).unwrap();
            if is_ascii_identifier_continue(ch) {
                continue;
            }
            if ch > MAX_ASCII {
                let cp = self.code_point_at(self.position).unwrap();
                if is_non_ascii_identifier_character(cp) {
                    return self.scan_non_ascii_identifier(cp);
                }
            }
            self.token = Token::Identifier;
            return Token::Identifier;
        }
    }

    fn scan_backticked_identifier(&mut self) -> Token {
        self.position += 1;
        self.token_flags.insert(TokenFlags::BACKTICKED);
        while !self.eof() {
            match self.char_at(self.position).unwrap() {
                0x5c => {
                    self.position += 2;
                    self.token_flags.insert(TokenFlags::ESCAPED);
                    continue;
                }
                0x60 => {
                    self.position += 1;
                    self.token = Token::Identifier;
                    return Token::Identifier;
                }
                CARRIAGE_RETURN | LINE_FEED => break,
                ch => {
                    if ch > MAX_ASCII {
                        self.token_flags.insert(TokenFlags::NON_ASCII);
                    }
                    self.position += 1;
                }
            }
        }
        self.unterminated(Token::Identifier)
    }

    fn scan_non_ascii_identifier(&mut self, start_code_point: u32) -> Token {
        self.token_flags.insert(TokenFlags::NON_ASCII);
        let mut cp = start_code_point;
        loop {
            self.position += utf16_code_units(cp);
            if self.eof() {
                break;
            }
            cp = self.code_point_at(self.position).unwrap();
            if !is_identifier_continue(cp) {
                break;
            }
        }
        self.token = Token::Identifier;
        Token::Identifier
    }

    fn at_conflict_marker(&self) -> bool {
        self.is_conflict_marker(self.position)
    }

    fn is_conflict_marker(&self, position: u32) -> bool {
        let ch = match self.char_at(position) {
            Some(ch) => ch,
            None => return false,
        };
        if !(position == 0 || self.char_at(position - 1).is_some_and(is_line_break)) {
            return false;
        }
        if position + MERGE_CONFLICT_MARKER_LENGTH >= self.end_position {
            return false;
        }
        for i in 0..MERGE_CONFLICT_MARKER_LENGTH {
            if self.char_at(position + i) != Some(ch) {
                return false;
            }
        }
        ch == 0x3d || self.char_at(position + MERGE_CONFLICT_MARKER_LENGTH) == Some(SPACE)
    }

    fn scan_conflict_marker(&mut self) -> Token {
        let marker = self.char_at(self.position).unwrap();
        self.position += MERGE_CONFLICT_MARKER_LENGTH;
        self.error(
            "conflict-marker",
            "Conflict marker encountered.",
            None,
            None,
        );
        if marker == 0x3c || marker == 0x3e {
            while !self.eof() && !is_line_break(self.char_at(self.position).unwrap()) {
                self.position += 1;
            }
        } else {
            while !self.eof() {
                let ch = self.char_at(self.position).unwrap();
                if (ch == 0x3d || ch == 0x3e)
                    && ch != marker
                    && self.is_conflict_marker(self.position)
                {
                    break;
                }
                self.position += 1;
            }
        }
        self.token = Token::ConflictMarker;
        Token::ConflictMarker
    }
}

pub fn scan_all(source: &SourceFile) -> (Vec<ScannedToken>, Vec<Diagnostic>) {
    let mut diagnostics = Vec::new();
    let mut scanner = Scanner::new(source, |diagnostic| diagnostics.push(diagnostic));
    let mut tokens = Vec::new();
    loop {
        let token = scanner.scan();
        let pos = scanner.token_position();
        let end = scanner.position();
        let value = scanner.token_value();
        tokens.push(ScannedToken {
            kind: token,
            pos,
            end,
            text: scanner.token_text(),
            value,
            flags: scanner.token_flags(),
        });
        if token == Token::EndOfFile {
            break;
        }
    }
    drop(scanner);
    (tokens, diagnostics)
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScannedToken {
    pub kind: Token,
    pub pos: u32,
    pub end: u32,
    pub text: String,
    pub value: Option<String>,
    pub flags: TokenFlags,
}

#[cfg(test)]
mod tests {
    use super::*;

    type TestToken = (Token, String, Option<String>, u32);

    fn scan(source: &str) -> (Vec<TestToken>, Vec<Diagnostic>) {
        let source = SourceFile::new(source.to_string());
        let (tokens, diagnostics) = scan_all(&source);
        (
            tokens
                .into_iter()
                .map(|x| (x.kind, x.text, x.value, x.pos))
                .collect(),
            diagnostics,
        )
    }

    #[test]
    fn scans_identifiers_keywords_punctuation() {
        let (tokens, diagnostics) = scan("model Foo{x:y}");
        assert!(diagnostics.is_empty());
        assert_eq!(
            tokens,
            vec![
                (Token::ModelKeyword, "model".into(), None, 0),
                (Token::Whitespace, " ".into(), None, 5),
                (Token::Identifier, "Foo".into(), Some("Foo".into()), 6),
                (Token::OpenBrace, "{".into(), None, 9),
                (Token::Identifier, "x".into(), Some("x".into()), 10),
                (Token::Colon, ":".into(), None, 11),
                (Token::Identifier, "y".into(), Some("y".into()), 12),
                (Token::CloseBrace, "}".into(), None, 13),
                (Token::EndOfFile, "".into(), None, 14),
            ]
        );
    }

    #[test]
    fn scans_numbers() {
        let (tokens, diagnostics) = scan("42 0xBEEF 0b1010 1.5e4 314.0e-2 1e+1000");
        assert!(diagnostics.is_empty());
        let numbers: Vec<_> = tokens
            .into_iter()
            .filter(|x| x.0 == Token::NumericLiteral)
            .map(|x| x.1)
            .collect();
        assert_eq!(
            numbers,
            ["42", "0xBEEF", "0b1010", "1.5e4", "314.0e-2", "1e+1000"]
        );
    }

    #[test]
    fn scans_string_values_and_escapes() {
        let (tokens, diagnostics) = scan("\"Hello \\r\\n \\t \\\" \\\\ !\"");
        assert!(diagnostics.is_empty());
        assert_eq!(tokens[0].0, Token::StringLiteral);
        assert_eq!(tokens[0].2.as_deref(), Some("Hello \r\n \t \" \\ !"));
    }

    #[test]
    fn reports_malformed_input() {
        let (_, diagnostics) = scan("\"unterminated");
        assert_eq!(diagnostics[0].code, "unterminated");
        assert_eq!(diagnostics[0].message, "Unterminated string literal.");
    }

    #[test]
    fn tracks_crlf_positions() {
        let source = SourceFile::new("a\r\nb".to_string());
        let (tokens, _) = scan_all(&source);
        let p = source.line_and_character(tokens[2].pos);
        assert_eq!(tokens[2].text, "b");
        assert_eq!(p.line, 1);
        assert_eq!(p.character, 0);
    }

    #[test]
    fn recognizes_doc_tokens() {
        let source = SourceFile::new("@tag `code`".to_string());
        let mut scanner = Scanner::new(&source, |_| {});
        assert_eq!(scanner.scan_doc(), Token::At);
        assert_eq!(scanner.scan_doc(), Token::Identifier);
        assert_eq!(scanner.scan_doc(), Token::Whitespace);
        assert_eq!(scanner.scan_doc(), Token::DocCodeSpan);
    }

    #[test]
    fn rescans_string_template_spans() {
        let source = SourceFile::new("\"a ${b} c\"".to_string());
        let mut scanner = Scanner::new(&source, |_| {});
        assert_eq!(scanner.scan(), Token::StringTemplateHead);
        let flags = scanner.token_flags();
        assert_eq!(scanner.token_value().as_deref(), Some("a "));
        assert_eq!(scanner.scan(), Token::Identifier);
        assert_eq!(scanner.scan(), Token::CloseBrace);
        assert_eq!(
            scanner.rescan_string_template(flags),
            Token::StringTemplateTail
        );
        assert_eq!(scanner.token_value().as_deref(), Some(" c"));
    }
}
