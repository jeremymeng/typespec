use napi_derive::napi;
use typespec_scanner::token::Token;
use typespec_scanner::{Scanner, TokenFlags};
use typespec_source::{Diagnostic, DiagnosticSeverity, SourceFile};

#[napi(object)]
pub struct TokenInfo {
    pub kind: u32,
    pub pos: u32,
    pub end: u32,
    pub line: u32,
    pub character: u32,
    pub text: String,
    pub value: Option<String>,
}

#[napi(object)]
pub struct DiagnosticInfo {
    pub severity: String,
    pub code: String,
    pub message: String,
    pub pos: u32,
    pub end: u32,
}

#[napi(object)]
pub struct ScanResult {
    pub tokens: Vec<TokenInfo>,
    pub diagnostics: Vec<DiagnosticInfo>,
}

struct TemplateContext {
    flags: TokenFlags,
    brace_depth: u32,
}

#[napi]
pub fn scan(source: String) -> ScanResult {
    let source = SourceFile::new(source);
    let mut diagnostics = Vec::new();
    let mut tokens = Vec::new();
    let mut scanner = Scanner::new(&source, |diagnostic| diagnostics.push(diagnostic));
    let mut templates: Vec<TemplateContext> = Vec::new();

    loop {
        let mut token = scanner.scan();

        if let Some(template) = templates.last_mut() {
            match token {
                Token::OpenBrace => {
                    template.brace_depth += 1;
                }
                Token::CloseBrace => {
                    if template.brace_depth == 0 {
                        let flags = template.flags;
                        token = scanner.rescan_string_template(flags);
                    } else {
                        template.brace_depth -= 1;
                    }
                }
                _ => {}
            }
        }

        let flags = scanner.token_flags();
        push_token(&source, &mut scanner, &mut tokens);

        match token {
            Token::StringTemplateHead => templates.push(TemplateContext {
                flags,
                brace_depth: 0,
            }),
            Token::StringTemplateMiddle => {
                if let Some(template) = templates.last_mut() {
                    template.flags = flags;
                    template.brace_depth = 0;
                }
            }
            Token::StringTemplateTail => {
                templates.pop();
            }
            Token::EndOfFile => break,
            _ => {}
        }
    }

    drop(scanner);
    ScanResult {
        tokens,
        diagnostics: diagnostics.into_iter().map(convert_diagnostic).collect(),
    }
}

fn push_token(source: &SourceFile, scanner: &mut Scanner<'_>, tokens: &mut Vec<TokenInfo>) {
    let pos = scanner.token_position();
    let end = scanner.position();
    let position = source.line_and_character(pos);
    tokens.push(TokenInfo {
        kind: scanner.token() as u32,
        pos,
        end,
        line: position.line,
        character: position.character,
        text: scanner.token_text(),
        value: scanner.token_value(),
    });
}

fn convert_diagnostic(diagnostic: Diagnostic) -> DiagnosticInfo {
    DiagnosticInfo {
        severity: match diagnostic.severity {
            DiagnosticSeverity::Error => "error",
            DiagnosticSeverity::Warning => "warning",
            DiagnosticSeverity::Hint => "hint",
        }
        .to_string(),
        code: diagnostic.code.to_string(),
        message: diagnostic.message,
        pos: diagnostic.target.pos,
        end: diagnostic.target.end,
    }
}
