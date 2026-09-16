//! Conflict pipeline (P8 PLAN): Git conflict → `ConflictParser` →
//! [`ConflictModel`] → editor. The CodeMirror editor only consumes the
//! model (block line ranges) and never parses markers itself; marker
//! semantics live here and are validated again server-side on resolve.
//!
//! Side contents are read from the index stages (`ls-files -u -z` gives
//! mode+sha per stage 1/2/3, then `cat-file blob <sha>`), which avoids all
//! `:N:path` rev-quoting pitfalls for exotic paths.

use super::IndexEntry;
use crate::core::error::AppError;
use serde::{Deserialize, Serialize};

/// Above this size a side/worktree is not loaded into a model; the file
/// routes to choose-side resolution only (protects IPC and renderer).
pub const MAX_SIDE_BYTES: u64 = 2 * 1024 * 1024;

/// Model-level conflict classification (PLAN P8: UI 据此路由).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, specta::Type)]
#[serde(rename_all = "snake_case")]
pub enum ConflictType {
    /// Both sides modified a text file — the inline conflict editor.
    Content,
    /// Deleted on one side, modified on the other (v1 also covers
    /// rename/delete: the resolution actions are equivalent).
    DeleteModify,
    /// Both sides added the file (no base or one-sided add).
    AddAdd,
    /// rename/rename(1to2): both sides renamed the same base path to
    /// different names (the two resulting paths are detected as a pair).
    /// v1 resolution is per-path keep/delete (选边).
    Rename,
    /// Kept for wire stability; v1 folds rename/delete into
    /// [`ConflictType::DeleteModify`] since keep/delete resolves both.
    RenameDelete,
    /// NUL byte on a relevant side — no editing, choose a side.
    Binary,
    /// The path is a file in one side's history and a directory in the
    /// worktree — choose-side only.
    DirectoryFile,
}

impl ConflictType {
    /// Short badge label key suffix (frontend i18n: `conflict.type.<key>`).
    pub fn key(self) -> &'static str {
        match self {
            ConflictType::Content => "content",
            ConflictType::DeleteModify => "delete_modify",
            ConflictType::AddAdd => "add_add",
            ConflictType::Rename => "rename",
            ConflictType::RenameDelete => "rename_delete",
            ConflictType::Binary => "binary",
            ConflictType::DirectoryFile => "directory_file",
        }
    }
}

/// One conflict region of the worktree text. All fields are 0-based line
/// indexes; `*_end` is exclusive.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, specta::Type)]
pub struct ConflictBlock {
    /// Line of the `<<<<<<<` marker.
    pub start: u32,
    /// First content line of the current (ours) side.
    pub current_start: u32,
    /// One past the last content line of the current side.
    pub current_end: u32,
    /// Lines of the diff3 base section (`|||||||`), when present.
    pub base_start: Option<u32>,
    pub base_end: Option<u32>,
    /// First content line of the incoming (theirs) side.
    pub incoming_start: u32,
    /// One past the last incoming line (equals `end`).
    pub incoming_end: u32,
    /// Line of the `>>>>>>>` marker.
    pub end: u32,
}

/// Lightweight per-path conflict info for the conflict file list.
#[derive(Debug, Clone, Serialize, Deserialize, specta::Type)]
pub struct ConflictSummary {
    pub path: String,
    /// XY unmerged code from the stage set (UU/AA/AU/UA/DU/UD).
    pub code: String,
    pub conflict_type: ConflictType,
    /// Number of parsed conflict blocks in the worktree text.
    pub block_count: u32,
    pub binary: bool,
    /// Any side/worktree exceeds [`MAX_SIDE_BYTES`].
    pub oversized: bool,
    /// Worktree path is a directory (DirectoryFile conflict).
    pub directory: bool,
    /// Stage mode is 160000 (submodule pointer conflict).
    pub submodule: bool,
    /// Whether the inline editor can open (text, sized, not binary/dir/sub).
    pub editable: bool,
}

/// Full model for one conflicted path (editor input).
#[derive(Debug, Clone, Serialize, Deserialize, specta::Type)]
pub struct ConflictModel {
    pub path: String,
    pub code: String,
    pub conflict_type: ConflictType,
    pub block_count: u32,
    pub binary: bool,
    pub oversized: bool,
    pub directory: bool,
    pub submodule: bool,
    pub editable: bool,
    /// Stage availability (base = stage 1, current = stage 2, incoming = 3).
    pub has_base: bool,
    pub has_current: bool,
    pub has_incoming: bool,
    /// Worktree document the editor opens — git's marker text as-is.
    /// `None` when unreadable / not UTF-8 / oversized.
    pub worktree_text: Option<String>,
    /// The worktree file uses CRLF line endings; the editor must convert
    /// (CodeMirror normalizes to LF) and the resolved text is converted
    /// back before writing.
    pub crlf: bool,
    pub base_text: Option<String>,
    pub current_text: Option<String>,
    pub incoming_text: Option<String>,
    pub blocks: Vec<ConflictBlock>,
}

// =====================
// Marker scanning (pure functions)
// =====================

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Marker {
    Start,
    Base,
    Sep,
    End,
}

/// git's marker rules (xdiff/xmerge.c): `<<<<<<<`/`|||||||`/`>>>>>>>` are
/// markers when exactly 7 chars followed by end-of-line or a space (labels
/// follow); `=======` must be exactly 7 equals. Trailing `\r` (CRLF files)
/// is ignored.
fn marker_at(line: &str) -> Option<Marker> {
    let l = line.strip_suffix('\r').unwrap_or(line);
    let classify = |pfx: &str| -> Option<Marker> {
        let rest = l.strip_prefix(pfx)?;
        if rest.is_empty() || rest.starts_with(' ') {
            Some(match pfx {
                "<<<<<<<" => Marker::Start,
                "|||||||" => Marker::Base,
                ">>>>>>>" => Marker::End,
                _ => unreachable!(),
            })
        } else {
            None
        }
    };
    classify("<<<<<<<")
        .or_else(|| classify(">>>>>>>"))
        .or_else(|| classify("|||||||"))
        .or_else(|| (l == "=======").then_some(Marker::Sep))
}

#[derive(Debug, Clone, Copy)]
enum ScanState {
    Idle,
    Ours,
    Base,
    Theirs,
}

/// Block under construction: start marker, ours start, base range, theirs start.
#[derive(Debug, Clone, Copy)]
struct PartialBlock {
    start: u32,
    current_start: u32,
    base: Option<(u32, u32)>,
    incoming_start: u32,
}

/// Scan worktree text for conflict blocks. Incomplete regions (EOF before
/// `>>>>>>>`) are dropped — the leftover-marker validation on resolve still
/// rejects such documents.
pub fn scan_blocks(text: &str) -> Vec<ConflictBlock> {
    let mut blocks = Vec::new();
    let mut state = ScanState::Idle;
    let mut cur: Option<PartialBlock> = None;
    for (i, line) in text.split('\n').enumerate() {
        let i = i as u32;
        // The final "" after a trailing \n is not a line.
        if i as usize == text.split('\n').count() - 1 && line.is_empty() && i > 0 {
            break;
        }
        match state {
            ScanState::Idle => {
                if marker_at(line) == Some(Marker::Start) {
                    cur = Some(PartialBlock {
                        start: i,
                        current_start: i + 1,
                        base: None,
                        incoming_start: i + 1,
                    });
                    state = ScanState::Ours;
                }
            }
            ScanState::Ours => match marker_at(line) {
                Some(Marker::Base) => {
                    if let Some(c) = cur.as_mut() {
                        c.base = Some((i + 1, i + 1));
                    }
                    state = ScanState::Base;
                }
                Some(Marker::Sep) => {
                    if let Some(c) = cur.as_mut() {
                        c.incoming_start = i + 1;
                    }
                    state = ScanState::Theirs;
                }
                Some(Marker::End) => {
                    // Empty ours AND empty theirs (git never emits this, but
                    // hand-edited files can): complete with empty sections.
                    if let Some(c) = cur.take() {
                        blocks.push(finish_block(c, i));
                    }
                    state = ScanState::Idle;
                }
                Some(Marker::Start) => {
                    // Nested/stray start: resync from here.
                    cur = Some(PartialBlock {
                        start: i,
                        current_start: i + 1,
                        base: None,
                        incoming_start: i + 1,
                    });
                }
                None => {}
            },
            ScanState::Base => match marker_at(line) {
                Some(Marker::Sep) => {
                    if let Some(c) = cur.as_mut() {
                        if let Some(b) = c.base.as_mut() {
                            b.1 = i;
                        }
                        c.incoming_start = i + 1;
                    }
                    state = ScanState::Theirs;
                }
                Some(Marker::Base) => {
                    // Stray second base marker: extend the base section.
                    if let Some(c) = cur.as_mut() {
                        if let Some(b) = c.base.as_mut() {
                            b.1 = i + 1;
                        }
                    }
                }
                Some(Marker::Start) | Some(Marker::End) => {
                    // Unterminated base section: drop and resync.
                    state = ScanState::Idle;
                    cur = None;
                    if marker_at(line) == Some(Marker::Start) {
                        cur = Some(PartialBlock {
                            start: i,
                            current_start: i + 1,
                            base: None,
                            incoming_start: i + 1,
                        });
                        state = ScanState::Ours;
                    }
                }
                None => {}
            },
            ScanState::Theirs => {
                if let Some(Marker::End) = marker_at(line) {
                    if let Some(c) = cur.take() {
                        blocks.push(finish_block(c, i));
                    }
                    state = ScanState::Idle;
                }
            }
        }
    }
    blocks
}

/// Complete a block at the end-marker line `end`.
fn finish_block(c: PartialBlock, end: u32) -> ConflictBlock {
    ConflictBlock {
        start: c.start,
        current_start: c.current_start,
        current_end: c
            .base
            .map_or(c.incoming_start.saturating_sub(1), |b| b.0 - 1),
        base_start: c.base.map(|b| b.0),
        base_end: c.base.map(|b| b.1),
        incoming_start: c.incoming_start,
        incoming_end: end,
        end,
    }
}

/// Lines that look like conflict start/end markers outside completed
/// blocks. Bare `=======` is deliberately NOT counted (legitimate setext
/// headings); git itself also never blocks commits on it.
fn leftover_marker_count(text: &str) -> usize {
    let mut in_block = false;
    let mut n = 0usize;
    for line in text.split('\n') {
        match marker_at(line) {
            Some(Marker::Start) if !in_block => {
                in_block = true;
                n += 1;
            }
            Some(Marker::End) if in_block => {
                in_block = false;
            }
            Some(Marker::End) => n += 1,
            _ => {}
        }
    }
    n
}

/// Resolve-time validation: a document may only be staged when no conflict
/// markers remain (PLAN P8: single source of marker truth on the Rust side).
pub fn validate_resolved(text: &str) -> Result<(), AppError> {
    let complete = scan_blocks(text).len();
    let leftover = leftover_marker_count(text);
    if complete == 0 && leftover == 0 {
        return Ok(());
    }
    Err(AppError::parse(format!(
        "conflict markers still present ({complete} blocks, {leftover} stray markers); \
         resolve every conflict before staging"
    )))
}

// =====================
// Classification (pure functions)
// =====================

/// XY unmerged code from the set of stages present (wt-status.c rule).
pub fn stage_code(stages: &[u32]) -> Option<&'static str> {
    let has = |s: u32| stages.contains(&s);
    match (has(1), has(2), has(3)) {
        (true, true, true) => Some("UU"),
        (false, true, true) => Some("AA"),
        (false, true, false) => Some("AU"),
        (false, false, true) => Some("UA"),
        (true, true, false) => Some("UD"),
        (true, false, true) => Some("DU"),
        (true, false, false) => Some("DD"), // auto-resolved; never listed
        (false, false, false) => None,
    }
}

/// NUL byte within the first 8 KiB → binary (same rule git uses).
pub fn looks_binary(bytes: &[u8]) -> bool {
    bytes.iter().take(8000).any(|&b| b == 0)
}

/// rename/rename(1to2) detection: a UD/DU path whose stage-1 (mode, sha)
/// equals another unmerged path's stage-1 record. The pair member carries
/// the same base blob. Plain modify/delete has no such twin and stays
/// [`ConflictType::DeleteModify`] (rename/delete folds in too — identical
/// resolution actions).
pub fn is_rename_pair(path: &str, records: &[IndexEntry], stages: &[(u32, IndexEntry)]) -> bool {
    use std::collections::HashSet;
    let code = stage_code(&stages.iter().map(|s| s.0).collect::<Vec<_>>());
    match code {
        // rename/rename(1to2) with base stages recorded at the new names
        Some("UD") | Some("DU") => {
            let Some(base) = stages.iter().find(|s| s.0 == 1) else {
                return false;
            };
            records.iter().any(|r| {
                r.path != path && r.stage == 1 && r.sha == base.1.sha && r.mode == base.1.mode
            })
        }
        // Common git shape: the two renamed paths carry stage 2 (AU) and
        // stage 3 (UA) of the SAME base blob; the old path shows as DD.
        Some("AU") | Some("UA") => {
            let own = stages
                .iter()
                .find(|(s, _)| code == Some("AU") && *s == 2)
                .map(|(_, e)| &e.sha)
                .or_else(|| stages.iter().find(|(s, _)| *s == 3).map(|(_, e)| &e.sha));
            let Some(own) = own else { return false };
            let other_stage = if code == Some("AU") { 3 } else { 2 };
            records
                .iter()
                .any(|r| r.path != path && r.stage == other_stage && &r.sha == own)
        }
        // The rename origin: stage 1 only, same blob as one of the new names.
        Some("DD") => {
            let Some(base) = stages.iter().find(|s| s.0 == 1) else {
                return false;
            };
            let twins: HashSet<&str> = records
                .iter()
                .filter(|r| r.stage == 2 || r.stage == 3)
                .map(|r| r.sha.as_str())
                .collect();
            twins.contains(base.1.sha.as_str())
        }
        _ => false,
    }
}

// =====================
// Engine-side assembly
// =====================

/// State of the worktree path as observed by the engine.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WorktreeState {
    /// Regular file, ≤ [`MAX_SIDE_BYTES`], bytes passed to the model.
    Ok,
    /// The path is a directory (DirectoryFile conflict).
    Directory,
    /// Regular file above [`MAX_SIDE_BYTES`].
    Oversized,
    /// Missing or unreadable (deleted on both sides, permissions…).
    Missing,
}

/// Read + classify one conflicted path into a [`ConflictModel`] (full) or a
/// [`ConflictSummary`] (texts omitted). Shared by the engine methods.
///
/// `stages` = this path's (stage, entry) records; `records` = all unmerged
/// records (rename-pair correlation); `worktree_bytes` is the capped file
/// content when `state` is [`WorktreeState::Ok`]; `side_bytes` holds the
/// stage contents (`None` = stage above the cap; stages absent from the vec
/// have no index entry).
pub fn build_model(
    path: &str,
    records: &[IndexEntry],
    stages: &[(u32, IndexEntry)],
    state: WorktreeState,
    worktree_bytes: Option<&[u8]>,
    // git's file/directory conflict puts the losing file at `<path>~<label>`
    // while the winning directory occupies the plain path.
    sibling_directory: bool,
    side_bytes: &[(u32, Option<Vec<u8>>)],
) -> ConflictModel {
    let code = stage_code(&stages.iter().map(|s| s.0).collect::<Vec<_>>())
        .unwrap_or("UU")
        .to_string();
    let directory = state == WorktreeState::Directory || sibling_directory;
    let submodule = stages.iter().any(|(_, e)| e.mode == "160000");
    let oversized =
        side_bytes.iter().any(|(_, b)| b.is_none()) || state == WorktreeState::Oversized;
    let binary = !directory
        && (stages
            .iter()
            .filter_map(|(s, _)| side_bytes.iter().find(|(bs, _)| bs == s))
            .any(|(_, b)| b.as_deref().is_some_and(looks_binary))
            || (state == WorktreeState::Ok && worktree_bytes.is_some_and(looks_binary)));
    let rename = is_rename_pair(path, records, stages);

    let conflict_type = if directory {
        ConflictType::DirectoryFile
    } else if submodule {
        // Submodule pointer conflicts cannot be edited or written; expose
        // them as choose-side/delete (v1: terminal-assisted).
        ConflictType::Binary
    } else if rename {
        ConflictType::Rename
    } else if binary {
        ConflictType::Binary
    } else {
        match code.as_str() {
            "UU" => ConflictType::Content,
            "AA" | "AU" | "UA" => ConflictType::AddAdd,
            "DU" | "UD" => ConflictType::DeleteModify,
            _ => ConflictType::Content,
        }
    };

    let worktree_text = worktree_bytes.and_then(|b| String::from_utf8(b.to_vec()).ok());
    let crlf = worktree_text.as_deref().is_some_and(|t| t.contains("\r\n"));
    // Non-UTF-8 worktree also blocks editing (rewriting would corrupt).
    // DirectoryFile: editing the `~<label>` parked file cannot resolve the
    // mapping (v1) — only choose/delete actions are offered.
    let editable = state == WorktreeState::Ok
        && worktree_text.is_some()
        && !binary
        && !oversized
        && !directory;

    let blocks = worktree_text
        .as_deref()
        .map(scan_blocks)
        .unwrap_or_default();

    ConflictModel {
        path: path.to_string(),
        code,
        conflict_type,
        block_count: blocks.len() as u32,
        binary,
        oversized,
        directory,
        submodule,
        editable,
        has_base: stages.iter().any(|(s, _)| *s == 1),
        has_current: stages.iter().any(|(s, _)| *s == 2),
        has_incoming: stages.iter().any(|(s, _)| *s == 3),
        worktree_text,
        crlf,
        base_text: side_text(side_bytes, stages, 1),
        current_text: side_text(side_bytes, stages, 2),
        incoming_text: side_text(side_bytes, stages, 3),
        blocks,
    }
}

fn side_text(
    side_bytes: &[(u32, Option<Vec<u8>>)],
    stages: &[(u32, IndexEntry)],
    stage: u32,
) -> Option<String> {
    stages.iter().find(|(s, _)| *s == stage)?;
    side_bytes
        .iter()
        .find(|(s, _)| *s == stage)
        .and_then(|(_, b)| b.as_ref().and_then(|b| String::from_utf8(b.clone()).ok()))
}

#[cfg(test)]
mod tests {
    use super::*;

    const MERGE_STYLE: &str =
        "top\n<<<<<<< HEAD\nours 1\nours 2\n=======\ntheirs 1\n>>>>>>> feature\nbottom\n";
    const DIFF3_STYLE: &str =
        "a\n<<<<<<< HEAD\nours\n||||||| base label\nbase\n=======\ntheirs\n>>>>>>> other\n";

    #[test]
    fn markers_match_git_rules() {
        assert_eq!(marker_at("<<<<<<< HEAD"), Some(Marker::Start));
        assert_eq!(marker_at("<<<<<<<"), Some(Marker::Start));
        assert_eq!(marker_at("<<<<<<<x"), None);
        assert_eq!(marker_at("======="), Some(Marker::Sep));
        assert_eq!(marker_at("========"), None);
        assert_eq!(marker_at("======= foo"), None);
        assert_eq!(marker_at(">>>>>>> feature\n"), Some(Marker::End));
        assert_eq!(marker_at(">>>>>>> feature\r"), Some(Marker::End));
        assert_eq!(
            marker_at("||||||| merged common ancestors"),
            Some(Marker::Base)
        );
        assert_eq!(marker_at("||||||||"), None);
        assert_eq!(marker_at("content"), None);
        // an exact ======= line matches even after a setext-style title —
        // harmless: the scanner ignores separators while Idle
        assert_eq!(marker_at("======="), Some(Marker::Sep));
    }

    #[test]
    fn scan_merge_style() {
        let blocks = scan_blocks(MERGE_STYLE);
        assert_eq!(blocks.len(), 1);
        let b = blocks[0];
        assert_eq!(b.start, 1);
        assert_eq!(b.current_start, 2);
        assert_eq!(b.current_end, 4);
        assert_eq!(b.base_start, None);
        assert_eq!(b.incoming_start, 5);
        assert_eq!(b.incoming_end, 6);
        assert_eq!(b.end, 6);
    }

    #[test]
    fn scan_diff3_style() {
        let blocks = scan_blocks(DIFF3_STYLE);
        assert_eq!(blocks.len(), 1);
        let b = blocks[0];
        assert_eq!(b.start, 1);
        assert_eq!(b.current_start, 2);
        assert_eq!(b.current_end, 3);
        assert_eq!(b.base_start, Some(4));
        assert_eq!(b.base_end, Some(5));
        assert_eq!(b.incoming_start, 6);
        assert_eq!(b.incoming_end, 7);
        assert_eq!(b.end, 7);
    }

    #[test]
    fn scan_multiple_and_trailing_newline() {
        let text =
            "<<<<<<< A\nx\n=======\ny\n>>>>>>> B\nmid\n<<<<<<< C\nz\n=======\nw\n>>>>>>> D\n";
        let blocks = scan_blocks(text);
        assert_eq!(blocks.len(), 2);
        assert_eq!(blocks[0].start, 0);
        assert_eq!(blocks[0].end, 4);
        assert_eq!(blocks[1].start, 6);
        assert_eq!(blocks[1].end, 10);
    }

    #[test]
    fn scan_incomplete_dropped() {
        // EOF before >>>>>>>: block dropped, leftovers caught at resolve.
        assert!(scan_blocks("<<<<<<< A\nx\n=======\ny\n").is_empty());
        assert!(scan_blocks("<<<<<<< A\nx\n").is_empty());
        assert!(scan_blocks("plain text").is_empty());
    }

    #[test]
    fn scan_crlf() {
        let blocks = scan_blocks("<<<<<<< A\r\nx\r\n=======\r\ny\r\n>>>>>>> B\r\n");
        assert_eq!(blocks.len(), 1);
        assert_eq!(blocks[0].current_start, 1);
        assert_eq!(blocks[0].incoming_start, 3);
    }

    #[test]
    fn validate_resolved_rejects_markers() {
        assert!(validate_resolved("clean text\n").is_ok());
        assert!(validate_resolved("clean with ======= underline\n").is_ok());
        assert!(validate_resolved(MERGE_STYLE).is_err());
        assert!(validate_resolved("<<<<<<< stray\n").is_err());
        assert!(validate_resolved(">>>>>>> stray\n").is_err());
        // resolved version of MERGE_STYLE keeping ours:
        assert!(validate_resolved("top\nours 1\nours 2\nbottom\n").is_ok());
    }

    fn entry(path: &str, sha: &str, stage: u32) -> IndexEntry {
        IndexEntry {
            path: path.to_string(),
            mode: "100644".into(),
            sha: sha.to_string(),
            stage,
        }
    }

    #[test]
    fn stage_codes() {
        assert_eq!(stage_code(&[1, 2, 3]), Some("UU"));
        assert_eq!(stage_code(&[2, 3]), Some("AA"));
        assert_eq!(stage_code(&[2]), Some("AU"));
        assert_eq!(stage_code(&[3]), Some("UA"));
        assert_eq!(stage_code(&[1, 2]), Some("UD"));
        assert_eq!(stage_code(&[1, 3]), Some("DU"));
        assert_eq!(stage_code(&[1]), Some("DD"));
        assert_eq!(stage_code(&[]), None);
    }

    #[test]
    fn build_model_content_conflict() {
        let records = vec![
            entry("f.txt", "base", 1),
            entry("f.txt", "ours", 2),
            entry("f.txt", "theirs", 3),
        ];
        let stages: Vec<(u32, IndexEntry)> = vec![
            (1, entry("f.txt", "base", 1)),
            (2, entry("f.txt", "ours", 2)),
            (3, entry("f.txt", "theirs", 3)),
        ];
        let sides = vec![
            (1, Some(b"base\n".to_vec())),
            (2, Some(b"ours\n".to_vec())),
            (3, Some(b"theirs\n".to_vec())),
        ];
        let m = build_model(
            "f.txt",
            &records,
            &stages,
            WorktreeState::Ok,
            Some(MERGE_STYLE.as_bytes()),
            false,
            &sides,
        );
        assert_eq!(m.code, "UU");
        assert_eq!(m.conflict_type, ConflictType::Content);
        assert!(m.editable);
        assert!(m.has_base && m.has_current && m.has_incoming);
        assert_eq!(m.block_count, 1);
        assert!(!m.crlf);
        assert_eq!(m.base_text.as_deref(), Some("base\n"));
    }

    #[test]
    fn build_model_delete_modify() {
        // theirs deleted: stages {1,2} = UD
        let records = vec![entry("f.txt", "base", 1), entry("f.txt", "ours", 2)];
        let stages = vec![
            (1, entry("f.txt", "base", 1)),
            (2, entry("f.txt", "ours", 2)),
        ];
        let sides = vec![(1, Some(b"base\n".to_vec())), (2, Some(b"ours\n".to_vec()))];
        let m = build_model(
            "f.txt",
            &records,
            &stages,
            WorktreeState::Ok,
            Some(b"ours\n"),
            false,
            &sides,
        );
        assert_eq!(m.code, "UD");
        assert_eq!(m.conflict_type, ConflictType::DeleteModify);
        assert!(!m.has_incoming);
        assert_eq!(m.incoming_text, None);
    }

    #[test]
    fn build_model_binary_and_oversize_and_directory() {
        let records = vec![entry("bin", "o", 2), entry("bin", "t", 3)];
        let stages = vec![(2, entry("bin", "o", 2)), (3, entry("bin", "t", 3))];

        // binary: the worktree file itself carries a NUL byte
        let sides = vec![(2, Some(b"x".to_vec())), (3, Some(b"y".to_vec()))];
        let m = build_model(
            "bin",
            &records,
            &stages,
            WorktreeState::Ok,
            Some(b"\0\0binary"),
            false,
            &sides,
        );
        assert_eq!(m.code, "AA");
        assert_eq!(m.conflict_type, ConflictType::Binary);
        assert!(m.binary);
        assert!(!m.editable);
        assert!(!m.directory);

        // oversized: a side above the cap comes through as None bytes
        let sides = vec![(2, None), (3, Some(b"x".to_vec()))];
        let m = build_model(
            "bin",
            &records,
            &stages,
            WorktreeState::Oversized,
            None,
            false,
            &sides,
        );
        assert!(m.oversized);
        assert!(!m.editable);

        // directory: path is a dir in the worktree, sides present and sized
        let sides = vec![(2, Some(b"a".to_vec())), (3, Some(b"b".to_vec()))];
        let m = build_model(
            "bin",
            &records,
            &stages,
            WorktreeState::Directory,
            None,
            false,
            &sides,
        );
        assert!(m.directory);
        assert!(!m.binary);
        assert_eq!(m.conflict_type, ConflictType::DirectoryFile);
    }

    #[test]
    fn build_model_rename_pair() {
        // base blob "base" renamed by both sides: UD at ours_name, DU at theirs
        let records = vec![
            entry("old.txt", "base", 1),
            entry("ours.txt", "base", 1),
            entry("ours.txt", "ours", 2),
            entry("theirs.txt", "base", 1),
            entry("theirs.txt", "theirs", 3),
        ];
        let stages_ours = vec![
            (1, entry("ours.txt", "base", 1)),
            (2, entry("ours.txt", "ours", 2)),
        ];
        let m = build_model(
            "ours.txt",
            &records,
            &stages_ours,
            WorktreeState::Ok,
            Some(b"ours\n"),
            false,
            &[(1, Some(b"base\n".to_vec())), (2, Some(b"ours\n".to_vec()))],
        );
        assert_eq!(m.conflict_type, ConflictType::Rename);

        // without the twin path this is a plain modify/delete
        let no_twin = vec![entry("ours.txt", "base", 1), entry("ours.txt", "ours", 2)];
        let m2 = build_model(
            "ours.txt",
            &no_twin,
            &stages_ours,
            WorktreeState::Ok,
            Some(b"ours\n"),
            false,
            &[(1, Some(b"base\n".to_vec())), (2, Some(b"ours\n".to_vec()))],
        );
        assert_eq!(m2.conflict_type, ConflictType::DeleteModify);
    }

    #[test]
    fn build_model_submodule_not_editable() {
        let mut sub = entry("sub", "c0ffee", 2);
        sub.mode = "160000".into();
        let mut sub3 = entry("sub", "f00d", 3);
        sub3.mode = "160000".into();
        let records = vec![sub.clone(), sub3.clone()];
        let stages = vec![(2, sub), (3, sub3)];
        let m = build_model(
            "sub",
            &records,
            &stages,
            WorktreeState::Directory,
            None,
            false,
            &[(2, Some(b"subdir".to_vec())), (3, Some(b"subdir".to_vec()))],
        );
        assert!(!m.editable);
        assert!(m.submodule);
        assert!(m.directory);
    }

    #[test]
    fn build_model_crlf_flag() {
        let records = vec![entry("f.txt", "o", 2), entry("f.txt", "t", 3)];
        let stages = vec![(2, entry("f.txt", "o", 2)), (3, entry("f.txt", "t", 3))];
        let sides = vec![(2, Some(b"a".to_vec())), (3, Some(b"b".to_vec()))];
        let m = build_model(
            "f.txt",
            &records,
            &stages,
            WorktreeState::Ok,
            Some(b"<<<<<<< A\r\nx\r\n=======\r\ny\r\n>>>>>>> B\r\n"),
            false,
            &sides,
        );
        assert!(m.crlf);
        assert_eq!(m.block_count, 1);
    }

    #[test]
    fn build_model_non_utf8_not_editable() {
        let records = vec![entry("f", "o", 2), entry("f", "t", 3)];
        let stages = vec![(2, entry("f", "o", 2)), (3, entry("f", "t", 3))];
        let sides = vec![(2, Some(b"a".to_vec())), (3, Some(b"b".to_vec()))];
        let m = build_model(
            "f",
            &records,
            &stages,
            WorktreeState::Ok,
            Some(&[0xC3, 0x28, b'\n']),
            false,
            &sides,
        );
        assert!(!m.editable);
        assert!(m.worktree_text.is_none());
        // invalid-UTF-8 alone must not look like a directory
        assert!(!m.directory);
        assert!(!m.oversized);
    }
}
