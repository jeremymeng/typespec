//! Program-wide source bookkeeping for multi-file compilation.
//!
//! Every loaded file is assigned a contiguous range in a single program-wide
//! position space (similar to rustc's `SourceMap`). AST nodes and diagnostics
//! store these global positions, so the binder and checker can work across
//! files without carrying a file id on every span. [`ProgramSources::locate`]
//! maps a global position back to a file and a file-local position.

use std::collections::HashMap;

use crate::{SourceFile, Span};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct SourceFileId(u32);

impl SourceFileId {
    pub fn new(value: u32) -> Self {
        Self(value)
    }

    pub fn index(self) -> usize {
        self.0 as usize
    }

    pub fn as_u32(self) -> u32 {
        self.0
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SourceFileKind {
    /// A parsed `.tsp` file.
    TypeSpec,
    /// A `.js`/`.mjs` import. Recorded for parity with TS but not executed.
    Js,
    /// A file only used as a diagnostic target (e.g. a library `package.json`).
    External,
}

#[derive(Debug, Clone)]
pub struct ProgramSourceFile {
    pub id: SourceFileId,
    /// Normalized absolute path (forward slashes).
    pub path: String,
    pub file: SourceFile,
    pub kind: SourceFileKind,
    /// Offset of this file's first position in the program position space.
    pub base: u32,
}

impl ProgramSourceFile {
    /// Exclusive end of this file's range in the program position space.
    pub fn end(&self) -> u32 {
        self.base + self.file.len_utf16()
    }
}

#[derive(Debug, Clone, Default)]
pub struct ProgramSources {
    files: Vec<ProgramSourceFile>,
    by_path: HashMap<String, SourceFileId>,
    entry: Option<SourceFileId>,
    next_base: u32,
}

impl ProgramSources {
    pub fn new() -> Self {
        Self::default()
    }

    /// Register a file. Returns the existing id if the path was already added.
    pub fn add(&mut self, path: String, file: SourceFile, kind: SourceFileKind) -> SourceFileId {
        if let Some(id) = self.by_path.get(&path) {
            return *id;
        }
        let id = SourceFileId::new(self.files.len() as u32);
        let base = self.next_base;
        // Leave a one-unit gap so a file's end position never equals the next
        // file's start position.
        self.next_base = base + file.len_utf16() + 1;
        self.by_path.insert(path.clone(), id);
        self.files.push(ProgramSourceFile {
            id,
            path,
            file,
            kind,
            base,
        });
        id
    }

    pub fn set_entry(&mut self, id: SourceFileId) {
        self.entry = Some(id);
    }

    /// Base offset the next added file will receive.
    pub fn next_base(&self) -> u32 {
        self.next_base
    }

    pub fn entry(&self) -> Option<SourceFileId> {
        self.entry
    }

    pub fn get(&self, id: SourceFileId) -> &ProgramSourceFile {
        &self.files[id.index()]
    }

    pub fn get_by_path(&self, path: &str) -> Option<&ProgramSourceFile> {
        self.by_path.get(path).map(|id| self.get(*id))
    }

    pub fn files(&self) -> &[ProgramSourceFile] {
        &self.files
    }

    pub fn len(&self) -> usize {
        self.files.len()
    }

    pub fn is_empty(&self) -> bool {
        self.files.is_empty()
    }

    /// Map a program-wide position to its file and file-local position.
    pub fn locate(&self, pos: u32) -> Option<(SourceFileId, u32)> {
        if pos == u32::MAX {
            return None;
        }
        let index = self.files.partition_point(|file| file.base <= pos);
        let file = self.files.get(index.checked_sub(1)?)?;
        (pos <= file.end()).then(|| (file.id, pos - file.base))
    }

    /// Map a program-wide span to its file and file-local span.
    pub fn locate_span(&self, span: Span) -> Option<(SourceFileId, Span)> {
        if span.is_no_target() {
            return None;
        }
        let (file, pos) = self.locate(span.pos)?;
        let base = self.get(file).base;
        Some((
            file,
            Span {
                pos,
                end: span.end.saturating_sub(base),
            },
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn assigns_disjoint_position_ranges() {
        let mut sources = ProgramSources::new();
        let a = sources.add(
            "/a.tsp".to_string(),
            SourceFile::new("model A {}".to_string()),
            SourceFileKind::TypeSpec,
        );
        let b = sources.add(
            "/b.tsp".to_string(),
            SourceFile::new("model B {}".to_string()),
            SourceFileKind::TypeSpec,
        );
        assert_eq!(sources.get(a).base, 0);
        assert_eq!(sources.get(b).base, 11);
        assert_eq!(sources.locate(0), Some((a, 0)));
        assert_eq!(sources.locate(10), Some((a, 10)));
        assert_eq!(sources.locate(11), Some((b, 0)));
        assert_eq!(sources.locate(17), Some((b, 6)));
        assert_eq!(sources.locate(u32::MAX), None);
        assert_eq!(
            sources.locate_span(Span { pos: 17, end: 18 }),
            Some((b, Span { pos: 6, end: 7 }))
        );
        assert_eq!(sources.locate_span(Span::NO_TARGET), None);
    }

    #[test]
    fn deduplicates_paths_and_counts_utf16() {
        let mut sources = ProgramSources::new();
        let a = sources.add(
            "/a.tsp".to_string(),
            SourceFile::new("😍".to_string()),
            SourceFileKind::TypeSpec,
        );
        let again = sources.add(
            "/a.tsp".to_string(),
            SourceFile::new(String::new()),
            SourceFileKind::TypeSpec,
        );
        assert_eq!(a, again);
        let b = sources.add(
            "/b.tsp".to_string(),
            SourceFile::new(String::new()),
            SourceFileKind::TypeSpec,
        );
        assert_eq!(sources.get(b).base, 3);
        assert_eq!(sources.get_by_path("/b.tsp").map(|file| file.id), Some(b));
    }
}
