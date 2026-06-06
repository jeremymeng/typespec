#![doc = "Shared source abstractions for the TypeSpec Rust port."]

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Position {
    pub line: u32,
    pub character: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Span {
    pub pos: u32,
    pub end: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum DiagnosticSeverity {
    Error,
    Warning,
    Hint,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Diagnostic {
    pub severity: DiagnosticSeverity,
    pub code: &'static str,
    pub message: String,
    pub target: Span,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SourceFile {
    text: String,
    line_starts: Vec<u32>,
}

impl SourceFile {
    pub fn new(text: String) -> Self {
        let line_starts = scan_line_starts(&text);
        Self { text, line_starts }
    }

    pub fn text(&self) -> &str {
        &self.text
    }

    pub fn line_starts(&self) -> &[u32] {
        &self.line_starts
    }

    pub fn line_and_character(&self, pos: u32) -> Position {
        let line = match self.line_starts.binary_search(&pos) {
            Ok(index) => index,
            Err(index) => index.saturating_sub(1),
        };
        let line_start = self.line_starts[line];
        Position {
            line: line as u32,
            character: pos.saturating_sub(line_start),
        }
    }
}

fn scan_line_starts(text: &str) -> Vec<u32> {
    let units: Vec<u16> = text.encode_utf16().collect();
    let mut starts = Vec::new();
    let mut start = 0u32;
    let mut pos = 0usize;

    while pos < units.len() {
        let ch = units[pos];
        pos += 1;
        match ch {
            0x0d => {
                if units.get(pos) == Some(&0x0a) {
                    pos += 1;
                }
                starts.push(start);
                start = pos as u32;
            }
            0x0a => {
                starts.push(start);
                start = pos as u32;
            }
            _ => {}
        }
    }

    starts.push(start);
    starts
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn maps_positions_across_lf_and_crlf() {
        let file = SourceFile::new("a\nbb\r\nccc\rd".to_string());
        assert_eq!(file.line_starts(), &[0, 2, 6, 10]);
        assert_eq!(
            file.line_and_character(0),
            Position {
                line: 0,
                character: 0
            }
        );
        assert_eq!(
            file.line_and_character(3),
            Position {
                line: 1,
                character: 1
            }
        );
        assert_eq!(
            file.line_and_character(7),
            Position {
                line: 2,
                character: 1
            }
        );
        assert_eq!(
            file.line_and_character(10),
            Position {
                line: 3,
                character: 0
            }
        );
    }

    #[test]
    fn counts_utf16_code_units() {
        let file = SourceFile::new("😍\na".to_string());
        assert_eq!(file.line_starts(), &[0, 3]);
        assert_eq!(
            file.line_and_character(2),
            Position {
                line: 0,
                character: 2
            }
        );
        assert_eq!(
            file.line_and_character(3),
            Position {
                line: 1,
                character: 0
            }
        );
    }

    #[test]
    fn bom_does_not_shift_line_starts() {
        let file = SourceFile::new("\u{feff}a\nb".to_string());
        assert_eq!(file.line_starts(), &[0, 3]);
        assert_eq!(
            file.line_and_character(1),
            Position {
                line: 0,
                character: 1
            }
        );
    }
}
