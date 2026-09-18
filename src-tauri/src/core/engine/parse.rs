//! Git output parsers.
//!
//! All parsers are pure functions over raw git output, unit-testable without
//! spawning processes (P1 acceptance: full parser unit-test coverage).

use super::{
    BackupRef, BlameCommit, BlameLine, BlameResult, BranchInfo, CommitFileStat, CommitInfo,
    DiffFile, DiffHunk, DiffLine, DiffLineKind, DiffModel, DiffSource, FileCommit, FileStatus,
    IndexEntry, NumstatCommit, NumstatFile, ReflogEntry, RemoteInfo, StashEntry, TagInfo,
};
use crate::core::error::AppError;

// =====================
// status --porcelain=v2 -z
// =====================

/// Parse `git status --porcelain=v2 -z` output.
///
/// Entry grammar (NUL-terminated):
/// - `1 <XY> <sub> <mH> <mI> <mW> <hH> <hI> <path>`            ordinary change
/// - `2 <XY> <sub> <mH> <mI> <mW> <hH> <hI> <X><score> <path>` rename/copy,
///   followed by a second NUL-separated field `<origPath>`
/// - `u <XY> <sub> <m1> <m2> <m3> <mW> <h1> <h2> <h3> <path>`   unmerged
/// - `? <path>`                                                 untracked
/// - `! <path>`                                                 ignored (skipped here)
pub fn parse_status(raw: &str) -> Vec<FileStatus> {
    let mut out = Vec::new();
    let parts: Vec<&str> = raw.split('\0').collect();
    let mut i = 0;
    while i < parts.len() {
        let rec = parts[i];
        if rec.is_empty() {
            i += 1;
            continue;
        }
        // Take the record and, for renames, the following origPath element.
        if let Some(rest) = rec.strip_prefix("2 ") {
            let orig = parts.get(i + 1).copied().unwrap_or("");
            i += 2;
            if let Some(fs) = parse_v2_rename(rest, orig) {
                out.push(fs);
            }
            continue;
        }
        i += 1;
        match rec.as_bytes().first() {
            Some(b'1') => {
                if let Some(fs) = parse_v2_ordinary(rec) {
                    out.push(fs);
                }
            }
            Some(b'u') => {
                if let Some(fs) = parse_v2_unmerged(rec) {
                    out.push(fs);
                }
            }
            Some(b'?') => {
                let path = rec[2..].to_string();
                if !path.is_empty() {
                    out.push(FileStatus {
                        path,
                        status: "?".into(),
                        orig_path: None,
                        submodule: false,
                        submodule_dirty: false,
                        submodule_commit_changed: false,
                        eol_only: false,
                        staged: false,
                        unstaged: false,
                        untracked: true,
                        skipped: false,
                        conflict: false,
                    });
                }
            }
            // `!` ignored entries and anything else are skipped.
            _ => {}
        }
    }
    out
}

fn parse_v2_ordinary(rec: &str) -> Option<FileStatus> {
    let rest = rec.strip_prefix("1 ")?;
    let mut it = rest.splitn(9, ' ');
    let xy = it.next()?;
    let sub = it.next()?;
    let _mh = it.next()?;
    let _mi = it.next()?;
    let _mw = it.next()?;
    let _hh = it.next()?;
    let _hi = it.next()?;
    let path = it.next()?.to_string();
    Some(build_file_status(xy, sub, path, None, false))
}

fn parse_v2_rename(rest: &str, orig: &str) -> Option<FileStatus> {
    let mut it = rest.splitn(10, ' ');
    let xy = it.next()?;
    let sub = it.next()?;
    let _mh = it.next()?;
    let _mi = it.next()?;
    let _mw = it.next()?;
    let _hh = it.next()?;
    let _hi = it.next()?;
    let _score = it.next()?; // e.g. "R100"
    let path = it.next()?.to_string();
    let orig_path = if orig.is_empty() {
        None
    } else {
        Some(orig.to_string())
    };
    Some(build_file_status(xy, sub, path, orig_path, false))
}

fn parse_v2_unmerged(rec: &str) -> Option<FileStatus> {
    let rest = rec.strip_prefix("u ")?;
    let mut it = rest.splitn(11, ' ');
    let xy = it.next()?;
    let sub = it.next()?;
    let _m1 = it.next()?;
    let _m2 = it.next()?;
    let _m3 = it.next()?;
    let _mw = it.next()?;
    let _h1 = it.next()?;
    let _h2 = it.next()?;
    let _h3 = it.next()?;
    let path = it.next()?.to_string();
    Some(build_file_status(xy, sub, path, None, true))
}

fn build_file_status(
    xy: &str,
    sub: &str,
    path: String,
    orig_path: Option<String>,
    conflict: bool,
) -> FileStatus {
    let x = xy.as_bytes().first().copied().unwrap_or(b'.') as char;
    let y = xy.as_bytes().get(1).copied().unwrap_or(b'.') as char;
    // Porcelain v2 `<sub>` field (git ≥2.40, verified on 2.54):
    // "N..." = not a submodule; "SC.." = submodule with a different
    // checked-in commit; "S..U"/"S.M." = submodule with untracked/modified
    // content. First char N/S decides, C/M/U flags carry the details.
    let sub_first = sub.as_bytes().first().copied().unwrap_or(b'N') as char;
    let submodule = sub_first == 'S';
    FileStatus {
        path,
        status: xy.to_string(),
        orig_path,
        submodule,
        submodule_dirty: submodule && (sub.contains('M') || sub.contains('U')),
        submodule_commit_changed: submodule && sub.contains('C'),
        eol_only: false,
        staged: conflict || (x != '.' && x != '?'),
        unstaged: conflict || (y != '.' && y != '?'),
        untracked: false,
        skipped: x == 'S' || y == 'S',
        conflict,
    }
}

// =====================
// ls-files -s -z
// =====================

/// Parse `git ls-files -s -z` output: `<mode> <sha> <stage>\t<path>` NUL-
/// separated records. Unmerged paths appear once per stage (1..3).
pub fn parse_ls_files(raw: &str) -> Vec<IndexEntry> {
    let mut out = Vec::new();
    for rec in raw.split('\0') {
        if rec.is_empty() {
            continue;
        }
        // mode SP sha SP stage TAB path
        let (meta, path) = match rec.split_once('\t') {
            Some(pair) => pair,
            None => continue,
        };
        let mut it = meta.split_whitespace();
        let (Some(mode), Some(sha), Some(stage)) = (it.next(), it.next(), it.next()) else {
            continue;
        };
        let Ok(stage) = stage.parse::<u32>() else {
            continue;
        };
        if path.is_empty() {
            continue;
        }
        out.push(IndexEntry {
            path: path.to_string(),
            mode: mode.to_string(),
            sha: sha.to_string(),
            stage,
        });
    }
    out
}

// =====================
// diff --numstat -z (path extraction)
// =====================

/// Extract the changed paths from `git diff --numstat -z` output.
///
/// Record layout (verified on git 2.54):
/// - ordinary: `<added>\t<removed>\t<path>\0` (TAB-separated, NUL-terminated)
/// - binary: counts are `-`
/// - rename/copy: `<added>\t<removed>\t\0<origPath>\0<path>\0` — the path
///   slot is empty and the two names follow as separate NUL fields.
pub fn parse_numstat_paths(raw: &str) -> Vec<String> {
    let mut out = Vec::new();
    let parts: Vec<&str> = raw.split('\0').collect();
    let mut i = 0;
    while i < parts.len() {
        let fields: Vec<&str> = parts[i].split('\t').collect();
        if fields.len() < 2 {
            i += 1;
            continue;
        }
        if fields.len() >= 3 && !fields[2].is_empty() {
            out.push(fields[2].to_string());
            i += 1;
        } else {
            // Rename layout: `<a>\t<r>\t` + `\0<origPath>\0<path>\0`.
            if let Some(path) = parts.get(i + 2) {
                if !path.is_empty() {
                    out.push(path.to_string());
                }
            }
            i += 3;
        }
    }
    out
}

// =====================
// log --numstat -z (P11 AI 报告采集)
// =====================

/// Parse `git log --all --numstat -z --format=%x1e%H%x1f%an%x1f%ae%x1f%aI%x1f%s`
/// into commit records with per-file stats.
///
/// Byte layout (verified on git 2.54):
/// ```text
/// \x1e<hash>\x1f<author>\x1f<email>\x1f<date>\x1f<subject>\0
/// <add>\t<del>\t<path>\0                      (ordinary)
/// <add>\t<del>\t\0<origPath>\0<path>\0        (rename/copy: path slot empty)
/// -\t-\t<path>\0                              (binary)
/// ```
/// The header is NUL-terminated (the `-z` mode terminator), followed by an
/// LF, then the NUL-separated numstat records; empty commits have nothing
/// after the header. `\x1e` (record separator) opens each commit; `\x1f`
/// (unit separator) delimits header fields. NUL/`\x1e` cannot occur in
/// commit messages; an `\x1f` inside a subject is tolerated (joined back).
pub fn parse_log_numstat(raw: &str) -> Vec<NumstatCommit> {
    let mut out = Vec::new();
    for record in raw.split('\x1e') {
        if record.is_empty() {
            continue;
        }
        let Some(nul) = record.find('\0') else {
            continue; // truncated tail record
        };
        let mut fields = record[..nul].split('\x1f');
        let hash = fields.next().unwrap_or("").to_string();
        if hash.is_empty() {
            continue;
        }
        let author = fields.next().unwrap_or("").to_string();
        let email = fields.next().unwrap_or("").to_string();
        let date = fields.next().unwrap_or("").to_string();
        // Subject is the last field; re-join if it contained \x1f.
        let subject = fields.collect::<Vec<_>>().join("\x1f");
        let files = parse_numstat_section(&record[nul + 1..]);
        out.push(NumstatCommit {
            short_hash: hash.chars().take(7).collect(),
            hash,
            author,
            email,
            date,
            subject,
            repo: String::new(),
            files,
        });
    }
    out
}

/// numstat 记录段：NUL 分隔的 token 流。普通记录 `<a>\t<r>\t<path>`；
/// rename 记录 path 槽为空，后两个 token 依次是 orig 与 new path。
fn parse_numstat_section(raw: &str) -> Vec<NumstatFile> {
    let raw = raw.trim_start_matches(['\n', '\r']);
    let mut files = Vec::new();
    let tokens: Vec<&str> = raw.split('\0').collect();
    let mut i = 0;
    while i < tokens.len() {
        let tok = tokens[i];
        i += 1;
        if tok.is_empty() {
            continue;
        }
        let fields: Vec<&str> = tok.split('\t').collect();
        if fields.len() < 3 {
            continue;
        }
        let binary = fields[0] == "-";
        let additions = fields[0].parse().unwrap_or(0);
        let deletions = fields[1].parse().unwrap_or(0);
        if fields[2].is_empty() {
            // Rename layout: `<a>\t<r>\t` + `\0<orig>\0<path>\0`。
            let orig = tokens.get(i).copied().unwrap_or("");
            let path = tokens.get(i + 1).copied().unwrap_or("");
            i += 2;
            if !path.is_empty() {
                files.push(NumstatFile {
                    path: path.to_string(),
                    orig_path: (!orig.is_empty()).then(|| orig.to_string()),
                    additions,
                    deletions,
                    binary,
                });
            }
        } else {
            files.push(NumstatFile {
                path: fields[2].to_string(),
                orig_path: None,
                additions,
                deletions,
                binary,
            });
        }
    }
    files
}

// =====================
// diff (unified)
// =====================

/// Parse unified `git diff` output into a [`DiffModel`].
///
/// Handles: add/delete/modify, rename & copy headers (`-M`), similarity index,
/// binary files, `\ No newline at end of file`.
///
/// Hunk bodies are consumed by remaining-line counters, so body lines that
/// happen to start with `---` / `+++` / `@@` (e.g. removed Markdown separators)
/// are never mistaken for file headers.
pub fn parse_diff(
    output: &str,
    source: DiffSource,
    rev_range: Option<(&str, &str)>,
) -> Result<DiffModel, AppError> {
    let mut files: Vec<DiffFile> = Vec::new();
    let mut cur: Option<DiffFile> = None;
    let mut cur_hunk: Option<DiffHunk> = None;
    // Remaining (old, new) body lines of the hunk currently being consumed.
    let mut hunk_left = (0u32, 0u32);
    // Running line numbers for the hunk being consumed; hunk header fields
    // keep their original values from the `@@` header.
    let mut line_no = (0u32, 0u32);

    let flush_hunk = |hunk: &mut Option<DiffHunk>| -> Option<DiffHunk> { hunk.take() };

    // NOTE: `str::lines()` strips a trailing `\r` (CRLF-aware) — that would
    // silently corrupt CRLF content and make reconstructed patches fail to
    // apply. Split on `\n` only; the `\r` stays part of the line content.
    let body = output.strip_suffix('\n').unwrap_or(output);
    for line in body.split('\n') {
        // `\ No newline at end of file` (LC_ALL=C locale is forced by the
        // runner): flags the preceding line as the no-trailing-newline
        // last line of its side. Consumes nothing else. The marker may
        // trail the body AFTER the remaining-line counters hit zero, so
        // this check runs before the body-consumption gate.
        if line.starts_with('\\') {
            if let Some(hunk) = cur_hunk.as_mut() {
                if let Some(last) = hunk.lines.last_mut() {
                    last.no_eol = true;
                }
            }
            continue;
        }
        // Inside a hunk: every line belongs to the body.
        if hunk_left.0 > 0 || hunk_left.1 > 0 {
            let Some(hunk) = cur_hunk.as_mut() else {
                hunk_left = (0, 0);
                continue;
            };
            if let Some(rest) = line.strip_prefix(' ') {
                hunk.lines.push(DiffLine {
                    content: rest.to_string(),
                    left_no: Some(line_no.0),
                    right_no: Some(line_no.1),
                    kind: DiffLineKind::Context,
                    no_eol: false,
                });
                line_no = (line_no.0 + 1, line_no.1 + 1);
                hunk_left.0 = hunk_left.0.saturating_sub(1);
                hunk_left.1 = hunk_left.1.saturating_sub(1);
            } else if let Some(rest) = line.strip_prefix('+') {
                hunk.lines.push(DiffLine {
                    content: rest.to_string(),
                    left_no: None,
                    right_no: Some(line_no.1),
                    kind: DiffLineKind::Add,
                    no_eol: false,
                });
                line_no.1 += 1;
                hunk_left.1 = hunk_left.1.saturating_sub(1);
            } else if let Some(rest) = line.strip_prefix('-') {
                hunk.lines.push(DiffLine {
                    content: rest.to_string(),
                    left_no: Some(line_no.0),
                    right_no: None,
                    kind: DiffLineKind::Remove,
                    no_eol: false,
                });
                line_no.0 += 1;
                hunk_left.0 = hunk_left.0.saturating_sub(1);
            }
            // Malformed lines consume nothing.
            continue;
        }

        if let Some(rest) = line.strip_prefix("diff --git ") {
            if let Some(h) = flush_hunk(&mut cur_hunk) {
                if let Some(f) = cur.as_mut() {
                    f.hunks.push(h);
                }
            }
            if let Some(prev) = cur.take() {
                files.push(prev);
            }
            let (op, np) = split_git_paths(rest);
            cur = Some(DiffFile {
                old_path: op,
                new_path: np,
                similarity: None,
                binary: false,
                hunks: Vec::new(),
            });
            continue;
        }
        let Some(file) = cur.as_mut() else {
            continue; // preamble lines before first "diff --git"
        };
        if let Some(sim) = line.strip_prefix("similarity index ") {
            let pct = sim.trim_end_matches('%').trim().parse().unwrap_or(0);
            file.similarity = Some(pct);
        } else if let Some(p) = line.strip_prefix("rename from ") {
            file.old_path = Some(unquote_path(p));
        } else if let Some(p) = line.strip_prefix("rename to ") {
            file.new_path = Some(unquote_path(p));
        } else if let Some(p) = line.strip_prefix("copy from ") {
            file.old_path = Some(unquote_path(p));
        } else if let Some(p) = line.strip_prefix("copy to ") {
            file.new_path = Some(unquote_path(p));
        } else if line.starts_with("Binary files ") || line.starts_with("GIT binary patch") {
            file.binary = true;
        } else if let Some(p) = line.strip_prefix("--- ") {
            let p = p.trim();
            file.old_path = decode_ab_path(p);
        } else if let Some(p) = line.strip_prefix("+++ ") {
            let p = p.trim();
            file.new_path = decode_ab_path(p);
        } else if let Some(h) = parse_hunk_header(line) {
            hunk_left = (h.old_count, h.new_count);
            line_no = (h.old_start, h.new_start);
            if let Some(prev) = flush_hunk(&mut cur_hunk) {
                file.hunks.push(prev);
            }
            cur_hunk = Some(h);
        }
    }
    if let Some(h) = flush_hunk(&mut cur_hunk) {
        if let Some(f) = cur.as_mut() {
            f.hunks.push(h);
        }
    }
    if let Some(last) = cur.take() {
        files.push(last);
    }

    Ok(DiffModel {
        id: 0,
        source,
        old_revision: rev_range.map(|(a, _)| a.to_string()),
        new_revision: rev_range.map(|(_, b)| b.to_string()),
        files,
    })
}

/// `a/old b/new` after `diff --git ` — only a fallback; ---/+++ lines are authoritative.
fn split_git_paths(rest: &str) -> (Option<String>, Option<String>) {
    if let Some(a) = rest.strip_prefix("a/") {
        // Find ` b/` separator at top level (paths may contain spaces).
        if let Some(idx) = a.find(" b/") {
            let old = &a[..idx];
            let new = a[idx + 3..].strip_prefix("b/").unwrap_or(&a[idx + 3..]);
            return (Some(old.to_string()), Some(new.to_string()));
        }
    }
    (None, None)
}

/// `--- a/path` / `+++ b/path` / `--- /dev/null` → Option<path>.
/// These lines are authoritative — they override the `diff --git` fallback.
fn decode_ab_path(s: &str) -> Option<String> {
    if s == "/dev/null" {
        return None;
    }
    let p = unquote_path(s);
    let p = p
        .strip_prefix("a/")
        .or_else(|| p.strip_prefix("b/"))
        .unwrap_or(&p);
    Some(p.to_string())
}

/// Undo git's C-style path quoting (`"..."` with backslash escapes / octal).
/// Octal escapes are raw UTF-8 bytes, so decode at the byte level.
fn unquote_path(p: &str) -> String {
    if !p.starts_with('"') || !p.ends_with('"') || p.len() < 2 {
        return p.to_string();
    }
    let inner = &p[1..p.len() - 1];
    let mut out: Vec<u8> = Vec::with_capacity(inner.len());
    let bytes = inner.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'\\' && i + 1 < bytes.len() {
            i += 1;
            match bytes[i] {
                b'n' => out.push(b'\n'),
                b't' => out.push(b'\t'),
                b'r' => out.push(b'\r'),
                b'"' => out.push(b'"'),
                b'\\' => out.push(b'\\'),
                b'0'..=b'7' => {
                    // Up to 3 octal digits.
                    let start = i;
                    let mut end = i;
                    while end < bytes.len()
                        && end < start + 3
                        && (b'0'..=b'7').contains(&bytes[end])
                    {
                        end += 1;
                    }
                    let val = u8::from_str_radix(&inner[start..end], 8).unwrap_or(b'?');
                    out.push(val);
                    i = end - 1;
                }
                c => out.push(c),
            }
        } else {
            out.push(bytes[i]);
        }
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// `@@ -l,c +l,c @@ ...` → DiffHunk.
fn parse_hunk_header(line: &str) -> Option<DiffHunk> {
    let rest = line.strip_prefix("@@ ")?;
    let end = rest.find(" @@")?;
    let ranges = &rest[..end];
    let header = rest[end + 3..].trim().to_string();
    let mut parts = ranges.split_whitespace();
    let old = parts.next()?.strip_prefix('-')?;
    let new = parts.next()?.strip_prefix('+')?;
    let (os, oc) = parse_range(old);
    let (ns, nc) = parse_range(new);
    Some(DiffHunk {
        old_start: os,
        old_count: oc,
        new_start: ns,
        new_count: nc,
        header,
        lines: Vec::new(),
    })
}

fn parse_range(s: &str) -> (u32, u32) {
    match s.split_once(',') {
        Some((a, b)) => (a.parse().unwrap_or(0), b.parse().unwrap_or(0)),
        None => (s.parse().unwrap_or(0), 1),
    }
}

// =====================
// log
// =====================

/// Parse `git log --format=%H%n%h%n%an%n%ae%n%aI%n%s%n%d%n%P` output:
/// 8 lines per commit, records are back-to-back.
pub fn parse_log(output: &str) -> Vec<CommitInfo> {
    let lines: Vec<&str> = output.lines().collect();
    let mut commits = Vec::new();
    let mut i = 0;
    while i + 8 <= lines.len() {
        let hash = lines[i].to_string();
        if hash.is_empty() {
            // Trailing garbage / blank separators — stop.
            break;
        }
        let short_hash = lines[i + 1].to_string();
        let author = lines[i + 2].to_string();
        let email = lines[i + 3].to_string();
        let date = lines[i + 4].to_string();
        let message = lines[i + 5].to_string();
        let refs = lines[i + 6]
            .trim_matches(|c| c == '(' || c == ')')
            .split(", ")
            .filter(|s| !s.is_empty())
            .map(|s| s.to_string())
            .collect::<Vec<_>>();
        let parents: Vec<String> = lines[i + 7]
            .split_whitespace()
            .map(|s| s.to_string())
            .collect();
        commits.push(CommitInfo {
            hash,
            short_hash,
            author,
            email,
            date,
            message,
            refs,
            parents,
        });
        i += 8;
    }
    commits
}

// =====================
// commit changed files (P5 提交详情)
// =====================

/// Parse `git diff-tree -r --root -M -z --name-status <hash>` output.
///
/// `-z` record shape (NUL-terminated fields, no quoting):
/// `"M" NUL "path" NUL` — or for renames/copies with score:
/// `"R93" NUL "src" NUL "dst" NUL`. The score rides on the status
/// token (`R100`, `C75`), which makes the two-path case unambiguous.
pub fn parse_commit_files(output: &str) -> Vec<CommitFileStat> {
    let mut toks = output.split('\0');
    let mut out = Vec::new();
    while let Some(st) = toks.next() {
        if st.is_empty() {
            // Trailing NUL or end of stream.
            break;
        }
        let letter = st.chars().next().unwrap_or('M').to_string();
        let score = st[letter.len()..].parse::<u32>().ok();
        let Some(path) = toks.next() else { break };
        if letter == "R" || letter == "C" {
            let Some(dst) = toks.next() else { break };
            out.push(CommitFileStat {
                status: letter,
                score,
                path: dst.to_string(),
                orig_path: Some(path.to_string()),
            });
        } else {
            out.push(CommitFileStat {
                status: letter,
                score: None,
                path: path.to_string(),
                orig_path: None,
            });
        }
    }
    out
}

// =====================
// branch (for-each-ref)
// =====================

/// Parse `git for-each-ref --format=%(refname)%00%(refname:short)%00%(upstream:short)%00%(upstream:track)%00%(HEAD)%00%(objectname)`.
pub fn parse_branches(output: &str) -> Vec<BranchInfo> {
    let mut out = Vec::new();
    for rec in output.lines() {
        if rec.is_empty() {
            continue;
        }
        let f: Vec<&str> = rec.split('\0').collect();
        if f.len() < 6 {
            continue;
        }
        let full_name = f[0].to_string();
        let name = f[1].to_string();
        let upstream = if f[2].is_empty() {
            None
        } else {
            Some(f[2].to_string())
        };
        let (ahead, behind) = parse_upstream_track(f[3]);
        let current = f[4] == "*";
        let hash = f[5].to_string();
        out.push(BranchInfo {
            name,
            full_name,
            upstream,
            ahead,
            behind,
            current,
            detached: hash.is_empty(),
        });
    }
    out
}

/// `[ahead 1, behind 2]` / `[ahead 2]` / `[behind 1]` / `[gone]` / ``.
fn parse_upstream_track(s: &str) -> (u32, u32) {
    let inner = s.trim_start_matches('[').trim_end_matches(']');
    let mut ahead = 0;
    let mut behind = 0;
    for part in inner.split(',') {
        let part = part.trim();
        if let Some(n) = part.strip_prefix("ahead ") {
            ahead = n.trim().parse().unwrap_or(0);
        } else if let Some(n) = part.strip_prefix("behind ") {
            behind = n.trim().parse().unwrap_or(0);
        }
    }
    (ahead, behind)
}

// =====================
// tag
// =====================

/// Parse `git tag -l --format=%(refname)%00%(refname:short)%00%(objectname)%00%(taggername)%00%(creatordate:iso)%00%(subject)`.
pub fn parse_tags(output: &str) -> Vec<TagInfo> {
    let mut out = Vec::new();
    for rec in output.lines() {
        if rec.is_empty() {
            continue;
        }
        let f: Vec<&str> = rec.split('\0').collect();
        if f.len() < 6 {
            continue;
        }
        out.push(TagInfo {
            full_name: f[0].to_string(),
            name: f[1].to_string(),
            target: f[2].to_string(),
            tagger: if f[3].is_empty() {
                None
            } else {
                Some(f[3].to_string())
            },
            date: if f[4].is_empty() {
                None
            } else {
                Some(f[4].to_string())
            },
            message: if f[5].is_empty() {
                None
            } else {
                Some(f[5].to_string())
            },
        });
    }
    out
}

// =====================
// stash
// =====================

/// Parse `git stash list --format=%H%00%gd%00%gs%00%cr`.
pub fn parse_stash(output: &str) -> Vec<StashEntry> {
    let mut out = Vec::new();
    for rec in output.lines() {
        if rec.is_empty() {
            continue;
        }
        let f: Vec<&str> = rec.split('\0').collect();
        if f.len() < 4 {
            continue;
        }
        let index = f[1]
            .rsplit_once('{')
            .and_then(|(_, rest)| rest.strip_suffix('}'))
            .and_then(|n| n.parse::<u32>().ok())
            .unwrap_or(0);
        let (branch, message) = split_stash_subject(f[2]);
        out.push(StashEntry {
            index,
            message,
            branch,
            date: f[3].to_string(),
        });
    }
    out
}

/// `"WIP on main: abc123 msg"` / `"On main: msg"` → (Some("main"), msg).
fn split_stash_subject(subject: &str) -> (Option<String>, String) {
    if let Some(rest) = subject.strip_prefix("WIP on ") {
        if let Some((branch, msg)) = rest.split_once(": ") {
            return (Some(branch.to_string()), msg.to_string());
        }
    } else if let Some(rest) = subject.strip_prefix("On ") {
        if let Some((branch, msg)) = rest.split_once(": ") {
            return (Some(branch.to_string()), msg.to_string());
        }
    }
    (None, subject.to_string())
}

// =====================
// remote
// =====================

/// Parse `git remote -v`: `name<TAB>url (fetch|push)` — merge fetch/push per name.
pub fn parse_remotes(output: &str) -> Vec<RemoteInfo> {
    let mut out: Vec<RemoteInfo> = Vec::new();
    for line in output.lines() {
        if line.is_empty() {
            continue;
        }
        let Some((name, rest)) = line.split_once('\t') else {
            continue;
        };
        let (url, kind) = match rest.rsplit_once(' ') {
            Some((u, "(fetch)")) => (u, 0),
            Some((u, "(push)")) => (u, 1),
            _ => (rest, 2),
        };
        if let Some(r) = out.iter_mut().find(|r| r.name == name) {
            if kind == 1 {
                r.push_url = url.to_string();
            } else if kind == 0 {
                r.fetch_url = url.to_string();
            }
        } else {
            let mut info = RemoteInfo {
                name: name.to_string(),
                url: String::new(),
                fetch_url: String::new(),
                push_url: String::new(),
            };
            if kind == 1 {
                info.push_url = url.to_string();
            } else {
                info.fetch_url = url.to_string();
            }
            out.push(info);
        }
    }
    out
}

// =====================
// reflog
// =====================

/// Parse `git reflog --format=%H%00%h%00%gd%00%gs%00%cr`.
pub fn parse_reflog(output: &str) -> Vec<ReflogEntry> {
    let mut out = Vec::new();
    for rec in output.lines() {
        if rec.is_empty() {
            continue;
        }
        let f: Vec<&str> = rec.split('\0').collect();
        if f.len() < 5 {
            continue;
        }
        // selector is like `HEAD@{0}` or `main@{2}` → ref part before `@{`.
        let ref_name = f[2]
            .split_once("@{")
            .map(|(r, _)| r.to_string())
            .unwrap_or_else(|| f[2].to_string());
        out.push(ReflogEntry {
            hash: f[0].to_string(),
            short_hash: f[1].to_string(),
            ref_name,
            message: f[3].to_string(),
            date: f[4].to_string(),
            author: String::new(),
        });
    }
    out
}

// =====================
// clean / backup refs (P6)
// =====================

/// Split a NUL-separated path list (`git ls-files -z` family) into paths.
/// Empty records are skipped; a trailing NUL yields no phantom entry.
pub fn parse_nul_paths(output: &str) -> Vec<String> {
    output
        .split('\0')
        .filter(|s| !s.is_empty())
        .map(|s| s.to_string())
        .collect()
}

/// Parse `git config --list -z` output (P10 Git 配置查看器): entries are
/// NUL-terminated, the first LF inside an entry separates key from value
/// (further LFs belong to the value — multi-line values are preserved).
/// A valueless (boolean-true) key is reported with an empty value.
pub fn parse_config_list_z(output: &str) -> Vec<(String, String)> {
    let mut out = Vec::new();
    for entry in output.split('\0') {
        if entry.is_empty() {
            continue;
        }
        match entry.split_once('\n') {
            Some((key, value)) => out.push((key.to_string(), value.to_string())),
            None => out.push((entry.to_string(), String::new())),
        }
    }
    out
}

/// Validate a `git config` key before it reaches argv (P10 常用配置编辑).
/// Accepts the common `section[.subsection].key` shape: ASCII alphanumerics
/// plus `.`/`-`, at least one `.` (section + key), no leading `-`/`.` and no
/// empty dot segments — defense against argument injection (`--unset` 等).
pub fn config_key_valid(key: &str) -> bool {
    !key.is_empty()
        && key
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '.' || c == '-')
        && !key.starts_with(['-', '.'])
        && !key.ends_with('.')
        && !key.contains("..")
        && key.contains('.')
}

/// Collapse a parsed config list into a `key → value` map with git's
/// "last occurrence wins" semantics (P10 仓库设置): `git config --get`
/// returns the last value of a multi-valued key, so the effective view of
/// `--list` output must too.
pub fn config_last_wins(
    entries: Vec<(String, String)>,
) -> std::collections::HashMap<String, String> {
    entries.into_iter().collect()
}

/// Parse `git rev-list --left-right --count A...B` → `(left, right)`.
pub fn parse_range_count(output: &str) -> (u32, u32) {
    let line = output.lines().next().unwrap_or("");
    let mut it = line.split_whitespace();
    let left = it.next().and_then(|v| v.parse().ok()).unwrap_or(0);
    let right = it.next().and_then(|v| v.parse().ok()).unwrap_or(0);
    (left, right)
}

/// Parse `git for-each-ref refs/ibexgit/backups/` with the backup-ref
/// format (refname, short name, objectname, short objectname,
/// creatordate:iso, subject — NUL separated).
pub fn parse_backup_refs(output: &str) -> Vec<BackupRef> {
    let mut out = Vec::new();
    for rec in output.lines() {
        if rec.is_empty() {
            continue;
        }
        let f: Vec<&str> = rec.split('\0').collect();
        if f.len() < 6 {
            continue;
        }
        // Display name: strip the fixed namespace from the full refname
        // (`refname:short` only strips `refs/`, which is noisy here).
        let name = f[0]
            .strip_prefix("refs/ibexgit/backups/")
            .unwrap_or(f[1])
            .to_string();
        out.push(BackupRef {
            full_name: f[0].to_string(),
            name,
            hash: f[2].to_string(),
            short_hash: f[3].to_string(),
            date: f[4].to_string(),
            subject: f[5].to_string(),
        });
    }
    out
}

// =====================
// file history --follow (P9)
// =====================

/// `true` when the token is a `--name-status` status marker: a single letter
/// (A/M/D/T/U) or a score-carrying rename/copy marker (`R93`, `C100`).
/// Status tokens never collide with the 40-hex commit marker that starts the
/// next record, which makes the interleaved format/name-status stream
/// unambiguous (paths are consumed positionally after each status token).
fn is_name_status_token(tok: &str) -> bool {
    matches!(tok, "A" | "M" | "D" | "T" | "U")
        || (tok.len() >= 2
            && (tok.starts_with('R') || tok.starts_with('C'))
            && tok[1..].bytes().all(|b| b.is_ascii_digit()))
}

fn is_hex40(tok: &str) -> bool {
    tok.len() == 40 && tok.bytes().all(|b| b.is_ascii_hexdigit())
}

/// Parse `git log --follow --format=%H%x00%h%x00%an%x00%ae%x00%aI%x00%s%x00%P
/// --name-status -z -- <path>`.
///
/// Record shape with `-z` (verified against git 2.54):
/// `<H> NUL <h> NUL <an> NUL <ae> NUL <aI> NUL <s> NUL <P> NUL LF`
/// then the name-status entries — `M NUL path NUL`, renames
/// `R100 NUL old NUL new NUL` — with no separator before the next record.
/// The record-terminator newline glues onto the first token after it and is
/// stripped; a new record begins at a 40-hex token.
pub fn parse_file_history(output: &str) -> Vec<FileCommit> {
    let mut toks = output
        .split('\0')
        .map(|t| t.strip_prefix('\n').unwrap_or(t))
        .peekable();
    let mut out = Vec::new();
    while let Some(hash) = toks.next() {
        if hash.is_empty() {
            continue;
        }
        if !is_hex40(hash) {
            break; // trailing garbage
        }
        let (Some(short_hash), Some(author), Some(email), Some(date), Some(message), Some(parents)) = (
            toks.next(),
            toks.next(),
            toks.next(),
            toks.next(),
            toks.next(),
            toks.next(),
        ) else {
            break;
        };

        // Name-status entries: consume complete groups while the next token
        // is a status marker. With a single filtered path there is exactly
        // one entry; keep the first for the model. R/C entries carry two
        // paths (old, new), all others exactly one — tokens are consumed
        // positionally so the next record's hash is never swallowed.
        let mut status = String::new();
        let mut path = String::new();
        let mut orig_path = None;
        let mut score = None;
        while toks
            .peek()
            .is_some_and(|t| !t.is_empty() && is_name_status_token(t))
        {
            let st = toks.next().unwrap();
            let Some(p1) = toks.next() else {
                break;
            };
            if status.is_empty() {
                let letter = st.chars().next().unwrap_or('M').to_string();
                score = st[letter.len()..].parse::<u32>().ok();
                status = letter.clone();
                if letter == "R" || letter == "C" {
                    orig_path = Some(p1.to_string());
                    path = toks.next().unwrap_or_default().to_string();
                } else {
                    path = p1.to_string();
                }
            } else if st.starts_with('R') || st.starts_with('C') {
                // Rarity guard: consume the extra path of later two-path
                // entries so the walk stays aligned (first entry kept).
                let _ = toks.next();
            }
        }
        if status.is_empty() {
            continue; // no entry for the filtered path — skip defensively
        }
        out.push(FileCommit {
            hash: hash.to_string(),
            short_hash: short_hash.to_string(),
            author: author.to_string(),
            email: email.to_string(),
            date: date.to_string(),
            message: message.to_string(),
            parents: parents.split_whitespace().map(|s| s.to_string()).collect(),
            path,
            orig_path,
            status,
            score,
        });
    }
    out
}

// =====================
// blame --porcelain (P9)
// =====================

/// sha git uses for working-tree lines that are not committed yet.
const BLAME_UNCOMMITTED_SHA: &str = "0000000000000000000000000000000000000000";

/// unix ts + `+HHMM` tz offset → ISO 8601 (git `%aI` style).
fn iso_from_ts_tz(ts: &str, tz: &str) -> String {
    let Ok(secs) = ts.parse::<i64>() else {
        return String::new();
    };
    use chrono::FixedOffset;
    let offset = (|| {
        let bytes = tz.as_bytes();
        if bytes.len() != 5 || (bytes[0] != b'+' && bytes[0] != b'-') {
            return None;
        }
        let hh: i32 = tz.get(1..3)?.parse().ok()?;
        let mm: i32 = tz.get(3..5)?.parse().ok()?;
        let sign = if bytes[0] == b'-' { -1 } else { 1 };
        FixedOffset::east_opt(sign * (hh * 3600 + mm * 60))
    })()
    // Unknown tz → UTC rather than failure.
    .or_else(|| FixedOffset::east_opt(0));
    chrono::DateTime::from_timestamp(secs, 0)
        .map(|utc| utc.with_timezone(&offset.unwrap()))
        .map(|dt| dt.to_rfc3339_opts(chrono::SecondsFormat::Secs, false))
        .unwrap_or_default()
}

/// Parse `git blame --porcelain -- <path>`.
///
/// Per line: a header `<sha> <origNo> <finalNo> [<groupSize>]`, then — only
/// for the first line of a commit — the metadata block (`author` …,
/// optional `boundary`, optional `previous`, terminated by `filename`),
/// then the content line prefixed with a TAB. Content lines never start
/// with a TAB-stripped header shape, and metadata keys are disjoint from
/// headers, so a single line walk is unambiguous.
pub fn parse_blame(output: &str) -> BlameResult {
    let mut out = BlameResult::default();
    let mut idx_by_sha: std::collections::HashMap<String, u32> = std::collections::HashMap::new();
    // split('\n') instead of lines(): content must keep CRLF fidelity
    // (see PLAN P4: str::lines() 会吞 \r).
    let mut it = output.split('\n').peekable();
    while let Some(header) = it.next() {
        if header.is_empty() {
            continue;
        }
        let mut parts = header.split(' ');
        let sha = parts.next().unwrap_or("");
        if !is_hex40(sha) {
            continue; // defensive: only commit headers start a record
        }
        let orig_no = parts.next().and_then(|v| v.parse().ok()).unwrap_or(0);
        let final_no = parts.next().and_then(|v| v.parse().ok()).unwrap_or(0);
        // group size (4th field) is redundant — ignored.

        let commit_idx = if let Some(i) = idx_by_sha.get(sha) {
            *i
        } else {
            let mut c = BlameCommit {
                hash: sha.to_string(),
                short_hash: sha[..7.min(sha.len())].to_string(),
                author: String::new(),
                email: String::new(),
                date: String::new(),
                summary: String::new(),
                path: String::new(),
                boundary: false,
                uncommitted: sha == BLAME_UNCOMMITTED_SHA,
            };
            let mut ts = String::new();
            let mut tz = String::new();
            // Metadata until `filename` (its terminator) or the next record.
            while let Some(line) = it.peek() {
                if line.starts_with('\t') {
                    break;
                }
                let mut kv = line.splitn(2, ' ');
                let key = kv.next().unwrap_or("");
                if is_hex40(key) {
                    break; // next record's header
                }
                let value = kv.next().unwrap_or("");
                it.next();
                match key {
                    "author" => c.author = value.to_string(),
                    "author-mail" => {
                        c.email = value.trim_matches(|c| c == '<' || c == '>').to_string()
                    }
                    "author-time" => ts = value.to_string(),
                    "author-tz" => tz = value.to_string(),
                    "summary" => c.summary = value.to_string(),
                    "filename" => c.path = value.to_string(),
                    "boundary" => c.boundary = true,
                    _ => {} // committer-* / previous — not needed by the UI
                }
            }
            c.date = iso_from_ts_tz(&ts, &tz);
            let idx = out.commits.len() as u32;
            out.commits.push(c);
            idx_by_sha.insert(sha.to_string(), idx);
            idx
        };

        // Content line (TAB-prefixed); strip one trailing CR (CRLF files).
        let content = match it.peek() {
            Some(l) if l.starts_with('\t') => {
                let l = it.next().unwrap();
                let raw = &l[1..];
                raw.strip_suffix('\r').unwrap_or(raw)
            }
            _ => "", // header without content (defensive; git always emits it)
        };
        out.lines.push(BlameLine {
            commit: commit_idx,
            orig_no,
            final_no,
            content: content.to_string(),
        });
    }
    out
}

// =====================
// tests
// =====================

#[cfg(test)]
mod tests {
    use super::*;

    // ---------- log --numstat (P11 AI 报告采集) ----------

    /// 与真实 git 输出字节一致的 fixture（含 binary / rename / 空提交）。
    const NUMSTAT_FIXTURE: &str = concat!(
        "\x1eb9771711aa88f5b6c441f3de51b3818ee395c48\x1fA\x1fa@x.com\x1f2026-09-17T09:21:42+08:00\x1fbinary\0\n",
        "-\t-\tbin.dat\0",
        "\x1e79d1cb4b5e28fa7b65addbc191b838ade7c8f0be\x1fA\x1fa@x.com\x1f2026-09-17T09:21:42+08:00\x1frename\0\n",
        "0\t0\t\0a.txt\0b.txt\0",
        "\x1e2eff30d5739d660e4234bed7bc3a4e15fc584ca1\x1fA\x1fa@x.com\x1f2026-09-17T09:21:42+08:00\x1fadd a.txt\0\n",
        "1\t0\ta.txt\0",
        "\x1efbcc88bca339d7f904775fb6f7441e116e2227\x1fA\x1fa@x.com\x1f2026-09-17T09:21:42+08:00\x1fempty commit\0",
    );

    #[test]
    fn log_numstat_binary_rename_and_empty() {
        let out = parse_log_numstat(NUMSTAT_FIXTURE);
        assert_eq!(out.len(), 4);
        // newest first (input order preserved)
        let binary = &out[0];
        assert_eq!(binary.subject, "binary");
        assert_eq!(binary.short_hash, "b977171");
        assert_eq!(binary.files.len(), 1);
        assert!(binary.files[0].binary);
        assert_eq!(binary.files[0].path, "bin.dat");

        let rename = &out[1];
        assert_eq!(rename.files.len(), 1);
        assert_eq!(rename.files[0].orig_path.as_deref(), Some("a.txt"));
        assert_eq!(rename.files[0].path, "b.txt");
        assert_eq!(
            (rename.files[0].additions, rename.files[0].deletions),
            (0, 0)
        );

        let add = &out[2];
        assert_eq!(add.files.len(), 1);
        assert_eq!((add.files[0].additions, add.files[0].deletions), (1, 0));

        let empty = &out[3];
        assert_eq!(empty.subject, "empty commit");
        assert!(empty.files.is_empty());
        // header fields round-trip
        assert_eq!(empty.author, "A");
        assert_eq!(empty.email, "a@x.com");
        assert_eq!(empty.date, "2026-09-17T09:21:42+08:00");
    }

    #[test]
    fn log_numstat_empty_and_truncated_input() {
        assert!(parse_log_numstat("").is_empty());
        // Truncated tail (no header NUL) is skipped, complete records kept.
        let raw = "\x1eabc\x1fA\x1fa@x\x1f2026-01-01T00:00:00+00:00\x1fs1\0\n1\t0\tf\0\x1etrunc";
        let out = parse_log_numstat(raw);
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].subject, "s1");
    }

    #[test]
    fn log_numstat_subject_with_unit_separator_is_rejoined() {
        let raw = "\x1eabc\x1fA\x1fa@x\x1f2026-01-01T00:00:00+00:00\x1ftitle\x1fpart\0\n1\t0\tf\0";
        let out = parse_log_numstat(raw);
        assert_eq!(out[0].subject, "title\x1fpart");
    }

    // ---------- status ----------

    #[test]
    fn status_v2_ordinary_modified() {
        // M. = staged modified; .M = unstaged modified
        let raw = "1 .M N... 100644 100644 100644 abc def src/main.rs\0";
        let out = parse_status(raw);
        assert_eq!(out.len(), 1);
        let f = &out[0];
        assert_eq!(f.path, "src/main.rs");
        assert_eq!(f.status, ".M");
        assert!(f.unstaged);
        assert!(!f.staged);
        assert!(!f.untracked);
        assert!(!f.conflict);
    }

    #[test]
    fn status_v2_staged_and_unstaged() {
        let raw = "1 MM N... 100644 100644 100644 abc def a.txt\0";
        let out = parse_status(raw);
        let f = &out[0];
        assert!(f.staged);
        assert!(f.unstaged);
    }

    #[test]
    fn status_v2_untracked_and_ignored() {
        let raw = "? new.txt\0! ignored.log\x001 .M N... 100644 100644 100644 a b tracked\0";
        let out = parse_status(raw);
        assert_eq!(out.len(), 2);
        assert_eq!(out[0].path, "new.txt");
        assert!(out[0].untracked);
        assert_eq!(out[1].path, "tracked");
    }

    #[test]
    fn status_v2_rename_with_orig_path() {
        // rename entries are `2 ... <path>\0<origPath>\0`
        let raw = "2 R. N... 100644 100644 100644 abc def R95 new.txt\0old.txt\0";
        let out = parse_status(raw);
        assert_eq!(out.len(), 1);
        let f = &out[0];
        assert_eq!(f.path, "new.txt");
        assert_eq!(f.orig_path.as_deref(), Some("old.txt"));
        assert_eq!(f.status, "R.");
        assert!(f.staged);
        assert!(!f.unstaged);
    }

    #[test]
    fn status_v2_unmerged_conflict() {
        let raw = "u AA N... 000000 100644 100644 000000 h1 h2 h3 conflict.txt\0";
        let out = parse_status(raw);
        let f = &out[0];
        assert_eq!(f.path, "conflict.txt");
        assert!(f.conflict);
        assert!(f.staged && f.unstaged);
    }

    #[test]
    fn status_v2_chinese_filename_raw_utf8() {
        // core.quotepath=false → raw UTF-8, no quoting
        let raw = "? 中文文件.txt\0";
        let out = parse_status(raw);
        assert_eq!(out[0].path, "中文文件.txt");
    }

    #[test]
    fn status_v2_skip_worktree() {
        let raw = "1 .S N... 100644 100644 100644 a b skip.txt\0";
        let out = parse_status(raw);
        assert!(out[0].skipped);
    }

    #[test]
    fn status_v2_submodule_states() {
        // sub field (real git output): N... = plain, S..U = untracked content
        // inside the submodule (dirty), SC.. = different checked-in commit.
        let raw = "1 .M S..U 160000 160000 160000 a b sub1\0\
                   1 .M N... 100644 100644 100644 a b file\0\
                   1 .M SC.. 160000 160000 160000 a b sub2\0";
        let out = parse_status(raw);
        assert_eq!(out.len(), 3);
        assert!(out[0].submodule && out[0].submodule_dirty);
        assert!(!out[0].submodule_commit_changed);
        assert!(!out[1].submodule && !out[1].submodule_dirty);
        assert!(out[2].submodule && out[2].submodule_commit_changed);
        assert!(!out[2].submodule_dirty);
    }

    // ---------- ls-files -s ----------

    #[test]
    fn ls_files_basic_and_unmerged() {
        let raw = "100644 1111111111111111111111111111111111111111 0\ta.txt\0\
                   100755 2222222222222222222222222222222222222222 0\tsh\0\
                   100644 3333333333333333333333333333333333333333 1\tc.txt\0\
                   100644 4444444444444444444444444444444444444444 2\tc.txt\0\
                   100644 5555555555555555555555555555555555555555 3\tc.txt\0";
        let out = parse_ls_files(raw);
        assert_eq!(out.len(), 5);
        assert_eq!(out[0].path, "a.txt");
        assert_eq!(out[0].mode, "100644");
        assert_eq!(out[0].stage, 0);
        assert_eq!(out[1].mode, "100755");
        let stages: Vec<u32> = out[2..]
            .iter()
            .filter(|e| e.path == "c.txt")
            .map(|e| e.stage)
            .collect();
        assert_eq!(stages, vec![1, 2, 3]);
    }

    // ---------- numstat -z ----------

    #[test]
    fn numstat_paths_basic_and_binary() {
        let raw = "1\t2\ta.txt\0-\t-\timg.png\x005\t0\tb.md\0";
        let out = parse_numstat_paths(raw);
        assert_eq!(out, vec!["a.txt", "img.png", "b.md"]);
    }

    #[test]
    fn numstat_paths_rename_empty_path_slot() {
        // rename layout: `1\t0\t\0orig\0new\0` (path slot empty)
        let raw = "1\t0\t\0old.txt\0new.txt\x003\t1\tok.txt\0";
        let out = parse_numstat_paths(raw);
        assert_eq!(out, vec!["new.txt", "ok.txt"]);
    }

    #[test]
    fn numstat_paths_chinese_filename_raw_utf8() {
        let raw = "1\t1\t中文.txt\0";
        let out = parse_numstat_paths(raw);
        assert_eq!(out, vec!["中文.txt"]);
    }

    // ---------- diff ----------

    #[test]
    fn diff_simple_modify() {
        // NOTE: `\x20` renders the mandatory context-line prefix space that the
        // Rust string continuation `\`+newline would otherwise strip.
        let raw = "diff --git a/a.txt b/a.txt\n\
                   index 111..222 100644\n\
                   --- a/a.txt\n\
                   +++ b/a.txt\n\
                   @@ -1,3 +1,3 @@ fn section\n\
                   \x20context line\n\
                   -old line\n\
                   +new line\n\
                   \x20another context\n";
        let model = parse_diff(raw, DiffSource::Worktree, None).unwrap();
        assert_eq!(model.files.len(), 1);
        let f = &model.files[0];
        assert_eq!(f.old_path.as_deref(), Some("a.txt"));
        assert_eq!(f.new_path.as_deref(), Some("a.txt"));
        assert_eq!(f.hunks.len(), 1);
        let h = &f.hunks[0];
        assert_eq!(h.old_start, 1);
        assert_eq!(h.new_start, 1);
        assert_eq!(h.header, "fn section");
        assert_eq!(h.lines.len(), 4);
        assert_eq!(h.lines[0].kind, DiffLineKind::Context);
        assert_eq!(h.lines[0].left_no, Some(1));
        assert_eq!(h.lines[0].right_no, Some(1));
        assert_eq!(h.lines[1].kind, DiffLineKind::Remove);
        assert_eq!(h.lines[1].left_no, Some(2));
        assert_eq!(h.lines[1].right_no, None);
        assert_eq!(h.lines[2].kind, DiffLineKind::Add);
        assert_eq!(h.lines[2].right_no, Some(2));
        assert_eq!(h.lines[3].left_no, Some(3));
    }

    #[test]
    fn diff_add_and_delete_file() {
        let raw = "diff --git a/added.txt b/added.txt\n\
                   new file mode 100644\n\
                   index 000..abc\n\
                   --- /dev/null\n\
                   +++ b/added.txt\n\
                   @@ -0,0 +1,2 @@\n\
                   +line one\n\
                   +line two\n\
                   diff --git a/deleted.txt b/deleted.txt\n\
                   deleted file mode 100644\n\
                   index abc..000\n\
                   --- a/deleted.txt\n\
                   +++ /dev/null\n\
                   @@ -1,1 +0,0 @@\n\
                   -goodbye\n";
        let model = parse_diff(raw, DiffSource::Staged, None).unwrap();
        assert_eq!(model.files.len(), 2);
        assert_eq!(model.files[0].old_path, None);
        assert_eq!(model.files[0].new_path.as_deref(), Some("added.txt"));
        assert_eq!(model.files[0].hunks[0].lines.len(), 2);
        assert_eq!(model.files[1].old_path.as_deref(), Some("deleted.txt"));
        assert_eq!(model.files[1].new_path, None);
        assert_eq!(model.files[1].hunks[0].lines[0].kind, DiffLineKind::Remove);
    }

    #[test]
    fn diff_rename_with_similarity() {
        let raw = "diff --git a/old_name.txt b/new_name.txt\n\
                   similarity index 92%\n\
                   rename from old_name.txt\n\
                   rename to new_name.txt\n\
                   index 111..222 100644\n\
                   --- a/old_name.txt\n\
                   +++ b/new_name.txt\n\
                   @@ -1 +1 @@\n\
                   -content\n\
                   +content2\n";
        let model = parse_diff(raw, DiffSource::Worktree, None).unwrap();
        let f = &model.files[0];
        assert_eq!(f.old_path.as_deref(), Some("old_name.txt"));
        assert_eq!(f.new_path.as_deref(), Some("new_name.txt"));
        assert_eq!(f.similarity, Some(92));
    }

    #[test]
    fn diff_binary_and_no_newline_marker() {
        let raw = "diff --git a/img.png b/img.png\n\
                   index 111..222 100644\n\
                   Binary files a/img.png and b/img.png differ\n\
                   diff --git a/x.txt b/x.txt\n\
                   index 333..444 100644\n\
                   --- a/x.txt\n\
                   +++ b/x.txt\n\
                   @@ -1 +1 @@\n\
                   -old\\n\n\
                   +new\n\
                   \\ No newline at end of file\n";
        let model = parse_diff(raw, DiffSource::Worktree, None).unwrap();
        assert_eq!(model.files.len(), 2);
        assert!(model.files[0].binary);
        // "\ No newline" line must not be counted as a diff line.
        assert_eq!(model.files[1].hunks[0].lines.len(), 2);
    }

    #[test]
    fn diff_body_line_starting_with_dashes_not_misparsed() {
        // A removed line whose content is "-- separator" appears as "--- separator"
        // and must stay a body line, not start a new file.
        let raw = "diff --git a/doc.md b/doc.md\n\
                   index 111..222 100644\n\
                   --- a/doc.md\n\
                   +++ b/doc.md\n\
                   @@ -1,3 +1,2 @@\n\
                   \x20title\n\
                   --- separator\n\
                   -old\n\
                   +new\n\
                   diff --git a/next.txt b/next.txt\n\
                   index 333..444 100644\n\
                   --- a/next.txt\n\
                   +++ b/next.txt\n\
                   @@ -1 +1 @@\n\
                   -a\n\
                   +b\n";
        let model = parse_diff(raw, DiffSource::Worktree, None).unwrap();
        assert_eq!(model.files.len(), 2);
        let f0 = &model.files[0];
        assert_eq!(f0.old_path.as_deref(), Some("doc.md"));
        let h = &f0.hunks[0];
        assert_eq!(h.lines.len(), 4);
        assert_eq!(h.lines[1].kind, DiffLineKind::Remove);
        assert_eq!(h.lines[1].content, "-- separator");
        assert_eq!(model.files[1].old_path.as_deref(), Some("next.txt"));
        assert_eq!(model.files[1].hunks.len(), 1);
    }

    #[test]
    fn diff_path_with_space_and_empty_context_line() {
        let raw = "diff --git a/sp ace.txt b/sp ace.txt\n\
                   index 111..222 100644\n\
                   --- a/sp ace.txt\n\
                   +++ b/sp ace.txt\n\
                   @@ -1,3 +1,3 @@\n\
                   \x20first\n\
                   \x20\n\
                   -third\n\
                   +changed\n";
        let model = parse_diff(raw, DiffSource::Worktree, None).unwrap();
        let f = &model.files[0];
        assert_eq!(f.old_path.as_deref(), Some("sp ace.txt"));
        let h = &f.hunks[0];
        // The blank context line is preserved.
        assert_eq!(h.lines[1].kind, DiffLineKind::Context);
        assert_eq!(h.lines[1].content, "");
        assert_eq!(h.lines.len(), 4);
    }

    #[test]
    fn diff_hunk_counts_advance_line_numbers() {
        let raw = "diff --git a/n.txt b/n.txt\n\
                   index 111..222 100644\n\
                   --- a/n.txt\n\
                   +++ b/n.txt\n\
                   @@ -10,3 +10,4 @@\n\
                   \x20a\n\
                   +b\n\
                   \x20c\n\
                   \x20d\n";
        let model = parse_diff(raw, DiffSource::Worktree, None).unwrap();
        let lines = &model.files[0].hunks[0].lines;
        assert_eq!(lines[0].left_no, Some(10));
        assert_eq!(lines[0].right_no, Some(10));
        assert_eq!(lines[1].right_no, Some(11));
        assert_eq!(lines[2].right_no, Some(12));
        assert_eq!(lines[2].left_no, Some(11));
    }

    #[test]
    fn diff_rev_range_recorded() {
        let model = parse_diff("", DiffSource::Commit, Some(("HEAD~1", "HEAD"))).unwrap();
        assert_eq!(model.old_revision.as_deref(), Some("HEAD~1"));
        assert_eq!(model.new_revision.as_deref(), Some("HEAD"));
    }

    #[test]
    fn diff_octal_escaped_path_unquoted() {
        // With quotepath=false non-ASCII comes raw, but quoted fallback still handled.
        // \\346\\226\\207 = UTF-8 bytes of 文.
        let raw = "diff --git a/\\346\\226\\207.txt b/\\346\\226\\207.txt\n\
                   index 111..222 100644\n\
                   --- \"a/\\346\\226\\207.txt\"\n\
                   +++ \"b/\\346\\226\\207.txt\"\n\
                   @@ -1 +1 @@\n\
                   -a\n\
                   +b\n";
        let model = parse_diff(raw, DiffSource::Worktree, None).unwrap();
        let f = &model.files[0];
        assert_eq!(f.old_path, Some("文.txt".to_string()));
    }

    #[test]
    fn diff_no_newline_marker_flags_lines() {
        // Old side: "old" without trailing newline; new side adds two lines.
        let raw = "diff --git a/x.txt b/x.txt\n\
                   index 333..444 100644\n\
                   --- a/x.txt\n\
                   +++ b/x.txt\n\
                   @@ -1 +1,2 @@\n\
                   -old\n\
                   \\ No newline at end of file\n\
                   +old\n\
                   +new\n";
        let model = parse_diff(raw, DiffSource::Worktree, None).unwrap();
        let lines = &model.files[0].hunks[0].lines;
        assert_eq!(lines.len(), 3);
        assert!(lines[0].no_eol, "remove line must be flagged");
        assert!(!lines[1].no_eol);
        assert!(!lines[2].no_eol);
    }

    #[test]
    fn diff_no_newline_on_both_sides_and_add_only() {
        // Both sides end without newline (context marker) + a pure-add tail.
        let raw = "diff --git a/y.txt b/y.txt\n\
                   index 333..444 100644\n\
                   --- a/y.txt\n\
                   +++ b/y.txt\n\
                   @@ -1 +1,2 @@\n\
                   \x20last\n\
                   \\ No newline at end of file\n\
                   +appended\n\
                   \\ No newline at end of file\n";
        let model = parse_diff(raw, DiffSource::Worktree, None).unwrap();
        let lines = &model.files[0].hunks[0].lines;
        assert_eq!(lines.len(), 2);
        assert!(lines[0].kind == DiffLineKind::Context && lines[0].no_eol);
        assert!(lines[1].kind == DiffLineKind::Add && lines[1].no_eol);
    }

    #[test]
    fn diff_worktree_only_change_removes_trailing_newline() {
        let raw = "diff --git a/z.txt b/z.txt\n\
                   index 333..444 100644\n\
                   --- a/z.txt\n\
                   +++ b/z.txt\n\
                   @@ -1 +1 @@\n\
                   -tail\n\
                   +tail\n\
                   \\ No newline at end of file\n";
        let model = parse_diff(raw, DiffSource::Worktree, None).unwrap();
        let lines = &model.files[0].hunks[0].lines;
        assert!(!lines[0].no_eol);
        assert!(lines[1].no_eol);
    }

    // ---------- log ----------

    #[test]
    fn log_two_commits_with_parents_and_refs() {
        let raw = "aaaa1111aaaa1111aaaa1111aaaa1111aaaa1111\n\
                   aaaa111\n\
                   Alice\n\
                   alice@example.com\n\
                   2024-01-01T10:00:00+08:00\n\
                   add feature\n\
                   (HEAD -> main, origin/main)\n\
                   \n\
                   bbbb2222bbbb2222bbbb2222bbbb2222bbbb2222\n\
                   bbbb222\n\
                   Bob\n\
                   bob@example.com\n\
                   2024-01-02T10:00:00+08:00\n\
                   initial\n\
                   \n\
                   aaaa1111aaaa1111aaaa1111aaaa1111aaaa1111\n";
        let out = parse_log(raw);
        assert_eq!(out.len(), 2);
        let c0 = &out[0];
        assert_eq!(c0.hash, "aaaa1111aaaa1111aaaa1111aaaa1111aaaa1111");
        assert_eq!(c0.author, "Alice");
        assert_eq!(c0.message, "add feature");
        assert_eq!(c0.refs, vec!["HEAD -> main", "origin/main"]);
        assert!(c0.parents.is_empty()); // root commit
        let c1 = &out[1];
        assert_eq!(c1.parents, vec!["aaaa1111aaaa1111aaaa1111aaaa1111aaaa1111"]);
        assert!(c1.refs.is_empty());
    }

    // ---------- branches ----------

    #[test]
    fn branches_parse_track_and_head() {
        let raw = "refs/heads/feature\0feature\0\0\0 \0abc1234abc1234abc1234abc1234abc1234\n\
                   refs/heads/main\0main\0origin/main\0[ahead 1, behind 2]\0*\0def5678def5678def5678def5678def5678\n";
        let out = parse_branches(raw);
        assert_eq!(out.len(), 2);
        let feat = &out[0];
        assert_eq!(feat.name, "feature");
        assert_eq!(feat.full_name, "refs/heads/feature");
        assert_eq!(feat.upstream, None);
        assert_eq!((feat.ahead, feat.behind), (0, 0));
        assert!(!feat.current);
        let main = &out[1];
        assert_eq!(main.upstream.as_deref(), Some("origin/main"));
        assert_eq!((main.ahead, main.behind), (1, 2));
        assert!(main.current);
    }

    #[test]
    fn branches_parse_gone_upstream() {
        let raw = "refs/heads/dev\0dev\0origin/dev\0[gone]\0 \0abc\n";
        let out = parse_branches(raw);
        let b = &out[0];
        assert_eq!((b.ahead, b.behind), (0, 0));
        assert_eq!(b.upstream.as_deref(), Some("origin/dev"));
    }

    // ---------- tags ----------

    #[test]
    fn tags_parse() {
        let raw =
            "refs/tags/v1.0\0v1.0\0abc1234full\0Alice\x002024-01-01 10:00:00 +0800\0release one\n\
                   refs/tags/light\0light\0def5678full\0\0\0\n";
        let out = parse_tags(raw);
        assert_eq!(out.len(), 2);
        let t = &out[0];
        assert_eq!(t.name, "v1.0");
        assert_eq!(t.full_name, "refs/tags/v1.0");
        assert_eq!(t.tagger.as_deref(), Some("Alice"));
        assert_eq!(t.message.as_deref(), Some("release one"));
        let light = &out[1];
        assert_eq!(light.tagger, None::<String>);
        assert_eq!(light.message, None::<String>);
    }

    // ---------- stash ----------

    #[test]
    fn stash_parse_wip_and_on() {
        let raw = "h1\0stash@{0}\0WIP on main: abc123 some commit\x002 hours ago\n\
                   h2\0stash@{1}\0On feature: manual message\0yesterday\n";
        let out = parse_stash(raw);
        assert_eq!(out.len(), 2);
        let s0 = &out[0];
        assert_eq!(s0.index, 0);
        assert_eq!(s0.branch.as_deref(), Some("main"));
        assert_eq!(s0.message, "abc123 some commit");
        let s1 = &out[1];
        assert_eq!(s1.index, 1);
        assert_eq!(s1.branch.as_deref(), Some("feature"));
        assert_eq!(s1.message, "manual message");
    }

    // ---------- remotes ----------

    #[test]
    fn remotes_parse_fetch_and_push() {
        let raw = "origin\thttps://example.com/a.git (fetch)\n\
                   origin\thttps://example.com/a.git (push)\n\
                   upstream\tgit@github.com:b/b.git (fetch)\n";
        let out = parse_remotes(raw);
        assert_eq!(out.len(), 2);
        let o = &out[0];
        assert_eq!(o.name, "origin");
        assert_eq!(o.fetch_url, "https://example.com/a.git");
        assert_eq!(o.push_url, "https://example.com/a.git");
        let u = &out[1];
        assert_eq!(u.name, "upstream");
        assert_eq!(u.fetch_url, "git@github.com:b/b.git");
        assert_eq!(u.push_url, "");
    }

    // ---------- reflog ----------

    #[test]
    fn reflog_parse() {
        let raw = "h1\0s1\0HEAD@{0}\0commit: fix bug\x002024-01-01 10:00:00 +0800\n\
                   h2\0s2\0HEAD@{1}\0checkout: moving from main to dev\x002024-01-01 09:00:00 +0800\n";
        let out = parse_reflog(raw);
        assert_eq!(out.len(), 2);
        let r0 = &out[0];
        assert_eq!(r0.hash, "h1");
        assert_eq!(r0.ref_name, "HEAD");
        assert_eq!(r0.message, "commit: fix bug");
        assert_eq!(r0.date, "2024-01-01 10:00:00 +0800");
    }

    // ---------- commit files (name-status -z) ----------

    use crate::core::engine::CommitFileStat;

    #[test]
    fn commit_files_parse_basic_and_rename() {
        // M, A, D and a rename with score; trailing NUL terminates.
        let raw = "M\0src/app.rs\0A\0new.txt\0D\0old.txt\0R93\0a.txt\0b.txt\0\0";
        let out = parse_commit_files(raw);
        assert_eq!(
            out,
            vec![
                CommitFileStat {
                    status: "M".into(),
                    score: None,
                    path: "src/app.rs".into(),
                    orig_path: None
                },
                CommitFileStat {
                    status: "A".into(),
                    score: None,
                    path: "new.txt".into(),
                    orig_path: None
                },
                CommitFileStat {
                    status: "D".into(),
                    score: None,
                    path: "old.txt".into(),
                    orig_path: None
                },
                CommitFileStat {
                    status: "R".into(),
                    score: Some(93),
                    path: "b.txt".into(),
                    orig_path: Some("a.txt".into())
                },
            ]
        );
    }

    #[test]
    fn commit_files_parse_empty_output() {
        assert!(parse_commit_files("").is_empty());
        assert!(parse_commit_files("\0").is_empty());
    }

    /// Chinese paths pass through raw (core.quotepath=false enforced at the
    /// runner level); typechange letters carry no score.
    #[test]
    fn commit_files_parse_unicode_and_typechange() {
        let raw = "T\0链接\0C75\0orig\\20name\0dst\0\0";
        let out = parse_commit_files(raw);
        assert_eq!(out.len(), 2);
        assert_eq!(out[0].status, "T");
        assert_eq!(out[0].path, "链接");
        assert_eq!(out[1].status, "C");
        assert_eq!(out[1].score, Some(75));
        assert_eq!(out[1].orig_path.as_deref(), Some("orig\\20name"));
    }

    // ---------- clean / backup refs (P6) ----------

    #[test]
    fn nul_paths_split_and_skip_empty() {
        let raw = "a.txt\0dir/\0中 文.txt\0\0";
        let out = parse_nul_paths(raw);
        assert_eq!(out, vec!["a.txt", "dir/", "中 文.txt"]);
        assert!(parse_nul_paths("").is_empty());
    }

    // ---------- config --list -z (P10 配置查看器) ----------

    #[test]
    fn config_list_z_basic_and_empty() {
        assert!(parse_config_list_z("").is_empty());
        // Trailing NUL yields no phantom entry.
        let raw = "core\nvalue with = and spaces\0user\n张三\0";
        let out = parse_config_list_z(raw);
        assert_eq!(
            out,
            vec![
                ("core".to_string(), "value with = and spaces".to_string()),
                ("user".to_string(), "张三".to_string()),
            ]
        );
    }

    #[test]
    fn config_list_z_multiline_and_valueless() {
        // Multi-line value: only the first LF separates key from value.
        // Valueless (boolean-true) key = no LF at all → empty value.
        let raw = "alias.lg\nlog --oneline\n --graph\0core.bare\0";
        let out = parse_config_list_z(raw);
        assert_eq!(
            out,
            vec![
                (
                    "alias.lg".to_string(),
                    "log --oneline\n --graph".to_string()
                ),
                ("core.bare".to_string(), String::new()),
            ]
        );
    }

    // ---------- config key validation (P10 常用配置编辑) ----------

    #[test]
    fn config_key_accepts_common_shapes() {
        for key in [
            "user.name",
            "user.email",
            "http.proxy",
            "https.proxy",
            "remote.origin.url",
            "remote.o-1.url", // subsection with digits/dashes
        ] {
            assert!(config_key_valid(key), "{key} should be valid");
        }
    }

    #[test]
    fn config_key_rejects_injection_and_malformed() {
        for key in [
            "",           // empty
            "username",   // no dot → no section separator
            "--unset",    // argument injection
            "-user.name", // leading dash
            ".user.name", // leading dot
            "user.name.", // trailing dot
            "user..name", // empty segment
            "user name",  // whitespace
            "user\nname", // control char
            "usér.name",  // non-ASCII
        ] {
            assert!(!config_key_valid(key), "{key:?} should be rejected");
        }
    }

    #[test]
    fn config_last_wins_takes_final_occurrence() {
        let entries = vec![
            ("user.name".to_string(), "first".to_string()),
            ("http.proxy".to_string(), "p1".to_string()),
            ("user.name".to_string(), "second".to_string()),
        ];
        let m = config_last_wins(entries);
        assert_eq!(m.get("user.name").map(String::as_str), Some("second"));
        assert_eq!(m.get("http.proxy").map(String::as_str), Some("p1"));
        assert!(!m.contains_key("absent"));
        assert!(config_last_wins(Vec::new()).is_empty());
    }

    #[test]
    fn range_count_parses_tab_and_space() {
        assert_eq!(parse_range_count("3\t7\n"), (3, 7));
        assert_eq!(parse_range_count("0 0"), (0, 0));
        assert_eq!(parse_range_count(""), (0, 0));
        assert_eq!(parse_range_count("garbage"), (0, 0));
    }

    #[test]
    fn backup_refs_parse() {
        // NOTE: `\0` must stay isolated — `\02024` would parse as octal.
        let raw = concat!(
            "refs/ibexgit/backups/reset-123\0reset-123\0h1\0s1\0",
            "2024-01-01 10:00:00 +0800\0",
            "commit: point\n",
            "refs/ibexgit/backups/rebase-456\0rebase-456\0h2\0s2\0\0\n"
        );
        let out = parse_backup_refs(raw);
        assert_eq!(out.len(), 2);
        let b = &out[0];
        assert_eq!(b.full_name, "refs/ibexgit/backups/reset-123");
        assert_eq!(b.name, "reset-123");
        assert_eq!(b.hash, "h1");
        assert_eq!(b.short_hash, "s1");
        assert_eq!(b.subject, "commit: point");
        assert_eq!(out[1].date, "");
        assert_eq!(out[1].subject, "");
    }
}

// =====================
// clone --progress (P7)
// =====================

/// One parsed progress line of `git clone --progress`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CloneProgress {
    /// Phase name (localized-independent English label from git itself):
    /// `receiving` | `resolving` | `updating` | `counting` | `compressing` |
    /// `enumerating` | other lowercased phase prefix.
    pub phase: String,
    /// 0..=100 (None when the line carries no percentage).
    pub percent: Option<u32>,
    /// `(current/total)` pair when present.
    pub current: Option<u64>,
    pub total: Option<u64>,
}

/// Parse one stderr progress line of `git clone --progress`:
/// `Receiving objects:  45% (123/273), 1.20 MiB | 2.31 MiB/s`.
/// Lines without a `<Phase>:` prefix (e.g. `Cloning into 'x'...`,
/// plain `remote:` footers) map to phase `other` with no numbers.
pub fn parse_clone_progress(line: &str) -> CloneProgress {
    let line = line.trim();
    // git 的 phase 名含空格（“Receiving objects”），不能用“无空格”判定；
    // 只要求冒号前非空且不含括号。
    let Some((head, rest)) = line.split_once(':') else {
        return CloneProgress {
            phase: "other".to_string(),
            percent: None,
            current: None,
            total: None,
        };
    };
    let head = head.trim();
    if head.is_empty() || head.contains('(') {
        return CloneProgress {
            phase: "other".to_string(),
            percent: None,
            current: None,
            total: None,
        };
    }
    if head.eq_ignore_ascii_case("remote") {
        // `remote: Counting objects: 100% (12/12), done.` → 解嵌套 phase。
        return match rest.trim().split_once(':') {
            Some((p, r)) if !p.trim().is_empty() => parse_progress_body(phase_word(p), r),
            _ => CloneProgress {
                phase: "other".to_string(),
                percent: None,
                current: None,
                total: None,
            },
        };
    }
    parse_progress_body(phase_word(head), rest)
}

/// Phase 名取首词小写：“Receiving objects” → “receiving”。
fn phase_word(p: &str) -> String {
    p.split_whitespace()
        .next()
        .unwrap_or("other")
        .to_lowercase()
}

fn parse_progress_body(phase: String, rest: &str) -> CloneProgress {
    let rest = rest.trim();
    let mut out = CloneProgress {
        phase,
        percent: None,
        current: None,
        total: None,
    };
    if let Some(pct) = rest.split_whitespace().next() {
        if let Some(num) = pct.strip_suffix('%') {
            if let Ok(v) = num.parse::<u32>() {
                out.percent = Some(v.min(100));
            }
        }
    }
    // (cur/total) pair — first parenthesis group with a slash.
    if let Some(open) = rest.find('(') {
        if let Some(close) = rest[open..].find(')') {
            let pair = &rest[open + 1..open + close];
            if let Some((c, t)) = pair.split_once('/') {
                if let (Ok(c), Ok(t)) = (c.trim().parse::<u64>(), t.trim().parse::<u64>()) {
                    out.current = Some(c);
                    out.total = Some(t);
                }
            }
        }
    }
    out
}

#[cfg(test)]
mod clone_progress_tests {
    use super::*;

    fn parse(line: &str) -> CloneProgress {
        parse_clone_progress(line)
    }

    #[test]
    fn receiving_objects_percent_and_pair() {
        let p = parse("Receiving objects:  45% (123/273), 1.20 MiB | 2.31 MiB/s");
        assert_eq!(p.phase, "receiving");
        assert_eq!(p.percent, Some(45));
        assert_eq!(p.current, Some(123));
        assert_eq!(p.total, Some(273));
    }

    #[test]
    fn resolving_and_updating() {
        let p = parse("Resolving deltas: 100% (45/45), done.");
        assert_eq!(p.phase, "resolving");
        assert_eq!(p.percent, Some(100));
        assert_eq!(p.total, Some(45));

        let p = parse("Updating files:  33% (4/12)");
        assert_eq!(p.phase, "updating");
        assert_eq!(p.percent, Some(33));
    }

    #[test]
    fn remote_prefix_is_unwrapped() {
        let p = parse("remote: Counting objects: 100% (12/12), done.");
        assert_eq!(p.phase, "counting");
        assert_eq!(p.percent, Some(100));

        let p = parse("remote: Enumerating objects: 12, done.");
        assert_eq!(p.phase, "enumerating");
        assert_eq!(p.percent, None);
    }

    #[test]
    fn header_lines_map_to_other() {
        let p = parse("Cloning into 'repo'...");
        assert_eq!(p.phase, "other");
        assert_eq!(p.percent, None);

        // `remote:` 后无第二段 phase。
        let p = parse("remote: Total 273 (delta 0), reused 0 (delta 0)");
        assert_eq!(p.phase, "other");
    }

    #[test]
    fn done_lines_without_percent() {
        let p = parse("Receiving objects: 100% (273/273), done.");
        assert_eq!(p.percent, Some(100));
    }
}

#[cfg(test)]
mod p9_file_trace_tests {
    use super::*;

    // Format under test (cli.rs::file_history):
    // %H%x00%h%x00%an%x00%ae%x00%aI%x00%s%x00%P + --name-status -z

    fn rec(hash: &str, author: &str, date: &str, msg: &str, parents: &str) -> String {
        format!("{hash}\0{hash}1\0{author}\0{author}@x\0{date}\0{msg}\0{parents}\0\n")
    }

    // ---------- parse_file_history ----------

    #[test]
    fn file_history_modify_only() {
        let raw = format!(
            "{}M\0src/a.rs\0{}M\0src/a.rs\0",
            rec(
                &"a".repeat(40),
                "Ann",
                "2026-01-02T10:00:00+08:00",
                "second",
                &"b".repeat(40)
            ),
            rec(
                &"b".repeat(40),
                "Bob",
                "2026-01-01T10:00:00+08:00",
                "first",
                ""
            )
        );
        let out = parse_file_history(&raw);
        assert_eq!(out.len(), 2);
        assert_eq!(out[0].hash, "a".repeat(40));
        assert_eq!(out[0].path, "src/a.rs");
        assert_eq!(out[0].status, "M");
        assert_eq!(out[0].orig_path, None);
        assert_eq!(out[0].parents, vec!["b".repeat(40)]);
        assert_eq!(out[1].status, "M");
        assert!(out[1].parents.is_empty());
    }

    #[test]
    fn file_history_rename_entry_carries_old_and_new_path() {
        // rename commit R100: status, old, new — then the next record.
        let raw = format!(
            "{}R100\0f.txt\0d/g.txt\0{}A\0f.txt\0",
            rec(
                &"a".repeat(40),
                "Ann",
                "2026-01-02T10:00:00+08:00",
                "rename",
                &"b".repeat(40)
            ),
            rec(
                &"b".repeat(40),
                "Bob",
                "2026-01-01T10:00:00+08:00",
                "first",
                ""
            )
        );
        let out = parse_file_history(&raw);
        assert_eq!(out.len(), 2);
        assert_eq!(out[0].status, "R");
        assert_eq!(out[0].score, Some(100));
        assert_eq!(out[0].orig_path.as_deref(), Some("f.txt"));
        assert_eq!(out[0].path, "d/g.txt");
        // pre-rename history keeps the old path
        assert_eq!(out[1].path, "f.txt");
    }

    #[test]
    fn file_history_chinese_and_typed_paths() {
        let raw = format!(
            "{}M\0中文/文档.md\0",
            rec(&"a".repeat(40), "安", "2026-01-01T00:00:00Z", "标题", "")
        );
        let out = parse_file_history(&raw);
        assert_eq!(out[0].path, "中文/文档.md");
    }

    #[test]
    fn file_history_skips_empty_token_and_stops_on_garbage() {
        // trailing NUL → empty token; non-hex token terminates the stream.
        let raw = format!("{}M\0a.rs\0\0", rec(&"a".repeat(40), "A", "t", "m", ""));
        let out = parse_file_history(&raw);
        assert_eq!(out.len(), 1);
        assert_eq!(parse_file_history("nonsense"), Vec::<FileCommit>::new());
    }

    #[test]
    fn file_history_subject_looking_like_hex_stays_positional() {
        // 40-hex *subject* is a positional format field — consumed, never
        // mistaken for the next record (which starts after the name-status).
        let subject = "F".repeat(40);
        let raw = format!(
            "{}M\0a.rs\0{}M\0a.rs\0",
            rec(&"a".repeat(40), "A", "t", &subject, &"b".repeat(40)),
            rec(&"b".repeat(40), "B", "t", "plain", "")
        );
        let out = parse_file_history(&raw);
        assert_eq!(out.len(), 2);
        assert_eq!(out[0].message, subject);
    }

    // ---------- parse_blame ----------

    const SHA1: &str = "1111111111111111111111111111111111111111";
    const SHA2: &str = "2222222222222222222222222222222222222222";

    fn meta(author: &str, ts: &str, tz: &str, summary: &str, path: &str) -> String {
        format!(
            "author {author}\nauthor-mail <{author}@x>\nauthor-time {ts}\nauthor-tz {tz}\n\
             committer {author}\ncommitter-mail <{author}@x>\ncommitter-time {ts}\n\
             committer-tz {tz}\nsummary {summary}\nboundary\nfilename {path}\n"
        )
    }

    #[test]
    fn blame_basic_attribution_and_dedup() {
        let raw = format!(
            "{SHA1} 1 1 1\n{}\tline one\n{SHA1} 2 2 1\n\tline two\n{SHA2} 3 3 1\n{}\tline three\n",
            meta("Ann", "1700000000", "+0800", "c1", "a.txt"),
            meta("Bob", "1700000100", "-0530", "c2", "a.txt"),
        );
        let out = parse_blame(&raw);
        assert_eq!(out.commits.len(), 2);
        assert_eq!(out.lines.len(), 3);
        assert_eq!(out.commits[0].hash, SHA1);
        assert_eq!(out.commits[0].author, "Ann");
        assert_eq!(out.commits[0].path, "a.txt");
        assert!(out.commits[0].boundary);
        assert!(!out.commits[0].uncommitted);
        // author ts+tz → ISO with offset
        assert_eq!(out.commits[0].date, "2023-11-15T06:13:20+08:00");
        assert_eq!(out.commits[1].date, "2023-11-14T16:45:00-05:30");
        // lines 1+2 share commit index 0; line 3 → commit 1
        assert_eq!(out.lines[0].commit, 0);
        assert_eq!(out.lines[1].commit, 0);
        assert_eq!(out.lines[2].commit, 1);
        assert_eq!(out.lines[0].content, "line one");
        assert_eq!(out.lines[2].orig_no, 3);
        assert_eq!(out.lines[2].final_no, 3);
    }

    #[test]
    fn blame_uncommitted_lines_use_zero_sha_pseudo_commit() {
        let raw = format!(
            "{SHA1} 1 1 1\n{}\tcommitted\n{BLAME_UNCOMMITTED_SHA} 2 2 1\n{}\tlocal edit\n",
            meta("Ann", "1700000000", "+0000", "c1", "a.txt"),
            "author Not Committed Yet\nauthor-mail <not.committed.yet>\n\
             author-time 1700000500\nauthor-tz +0000\nsummary Version of a.txt\n\
             filename a.txt\n",
        );
        let out = parse_blame(&raw);
        assert_eq!(out.commits.len(), 2);
        assert!(!out.commits[0].uncommitted);
        assert!(out.commits[1].uncommitted);
        assert_eq!(out.lines[1].commit, 1);
        assert_eq!(out.lines[1].content, "local edit");
    }

    #[test]
    fn blame_preserves_crlf_and_empty_content() {
        // CRLF file: git emits "\tcontent\r\n" — the trailing CR is stripped
        // for display; a genuinely empty content line stays empty. The
        // second line re-uses the seen commit (no metadata block repeat).
        let raw = format!(
            "{SHA1} 1 1 2\n{}\tfirst\r\n{SHA1} 2 2 2\n\t\n",
            meta("Ann", "1700000000", "+0000", "c1", "a.txt"),
        );
        let out = parse_blame(&raw);
        assert_eq!(out.lines.len(), 2);
        assert_eq!(out.commits.len(), 1);
        assert_eq!(out.lines[0].content, "first");
        assert_eq!(out.lines[1].content, "");
        assert_eq!(out.lines[1].commit, 0);
    }

    #[test]
    fn blame_empty_file_yields_empty_result() {
        assert_eq!(parse_blame(""), BlameResult::default());
    }

    #[test]
    fn blame_iso_fallback_on_bad_ts() {
        assert_eq!(iso_from_ts_tz("nope", "+0800"), "");
        // unknown tz → UTC ISO rather than failure
        assert_eq!(iso_from_ts_tz("0", "xxxxx"), "1970-01-01T00:00:00+00:00");
    }
}

// =====================
// stderr failure classification: the dirty-worktree family
// =====================

/// Structured reason for a refused worktree-mutating operation: git aborted
/// because uncommitted (or untracked) local content would be overwritten.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DirtyWorktree {
    /// Operation git refused, as printed by git itself:
    /// `merge` | `checkout` | `switch` | `pull` | `rebase`.
    pub operation: String,
    /// Paths git listed as conflicting; empty when git names none
    /// (e.g. `cannot pull with rebase: You have unstaged changes.`).
    pub files: Vec<String>,
    /// True when the blocked paths are *untracked* files
    /// (`The following untracked working tree files would be overwritten…`).
    /// A plain `git stash` does not clear untracked files, so the UI must
    /// not offer stash-and-retry for this shape.
    pub untracked: bool,
}

/// Marker common to both the tracked and untracked overwrite refusals.
const OVERWRITTEN_MARKER: &str = "would be overwritten by ";

/// Recognize the "your local changes would be overwritten" family in git's
/// stderr and extract the refused operation + conflicting paths.
///
/// Covered shapes (git always runs under `LC_ALL=C.UTF-8`, see runner, so
/// the messages are reliably English):
/// - `error: Your local changes to the following files would be overwritten
///    by <op>:` + indented path list + `Please commit your changes or stash
///    them before you ….` (merge / checkout / switch / pull)
/// - `error: The following untracked working tree files would be overwritten
///    by <op>:` + path list (same tail)
/// - `error: cannot pull with rebase: You have unstaged changes.`
/// - `error: cannot pull with rebase: Your index contains uncommitted changes.`
/// - `error: cannot rebase: You have unstaged changes.`
///
/// Returns `None` for anything else (caller keeps the raw stderr path).
pub fn parse_dirty_worktree(stderr: &str) -> Option<DirtyWorktree> {
    if let Some(pos) = stderr.find(OVERWRITTEN_MARKER) {
        let rest = &stderr[pos + OVERWRITTEN_MARKER.len()..];
        let end = rest.find(':')?;
        let operation = rest[..end].trim().to_string();
        if operation.is_empty() {
            return None;
        }
        let untracked = stderr[..pos].contains("untracked working tree files");
        let files: Vec<String> = rest[end + 1..]
            .lines()
            .map(|l| l.trim())
            .skip_while(|l| l.is_empty())
            .take_while(|l| !l.is_empty() && !l.starts_with("Please "))
            .filter(|l| !l.starts_with("error:"))
            .map(|l| l.to_string())
            .collect();
        if !files.is_empty() {
            return Some(DirtyWorktree {
                operation,
                files,
                untracked,
            });
        }
    }
    // Rebase-family refusals name no files, only the cause.
    const REBASE_REFUSALS: [(&str, &str); 4] = [
        (
            "cannot pull with rebase: You have unstaged changes.",
            "pull",
        ),
        (
            "cannot pull with rebase: Your index contains uncommitted changes.",
            "pull",
        ),
        ("cannot rebase: You have unstaged changes.", "rebase"),
        (
            "cannot rebase: Your index contains uncommitted changes.",
            "rebase",
        ),
    ];
    for (needle, operation) in REBASE_REFUSALS {
        if stderr.contains(needle) {
            return Some(DirtyWorktree {
                operation: operation.to_string(),
                files: Vec::new(),
                untracked: false,
            });
        }
    }
    None
}

#[cfg(test)]
mod dirty_worktree_tests {
    use super::parse_dirty_worktree;

    /// Real-world shape (codeup.aliyun.com pull refused by a dirty vue file).
    #[test]
    fn pull_merge_with_file_list() {
        let stderr = "From codeup.aliyun.com:64264d90eafb57df57532d6c/saas/hs-saas-tms-web\n\
                      * branch            dev        -> FETCH_HEAD\n\
                      error: Your local changes to the following files would be overwritten by merge:\n\
                      \tsrc/views/cms/customsbroker/declarationhead.vue\n\
                      Please commit your changes or stash them before you merge.\n\
                      Aborting";
        let d = parse_dirty_worktree(stderr).expect("classified");
        assert_eq!(d.operation, "merge");
        assert!(!d.untracked);
        assert_eq!(
            d.files,
            vec!["src/views/cms/customsbroker/declarationhead.vue"]
        );
    }

    #[test]
    fn checkout_multiple_files() {
        let stderr =
            "error: Your local changes to the following files would be overwritten by checkout:\n\
                      \tsrc/a.ts\n\
                      \tsrc/b.rs\n\
                      Please commit your changes or stash them before you switch branches.\n\
                      Aborting";
        let d = parse_dirty_worktree(stderr).expect("classified");
        assert_eq!(d.operation, "checkout");
        assert_eq!(d.files, vec!["src/a.ts", "src/b.rs"]);
    }

    #[test]
    fn untracked_variant_is_flagged() {
        let stderr =
            "error: The following untracked working tree files would be overwritten by merge:\n\
                      \tconfig/local.json\n\
                      Please move or remove them before you merge.\n\
                      Aborting";
        let d = parse_dirty_worktree(stderr).expect("classified");
        assert_eq!(d.operation, "merge");
        assert!(d.untracked);
        assert_eq!(d.files, vec!["config/local.json"]);
    }

    #[test]
    fn rebase_refusal_without_file_list() {
        let d = parse_dirty_worktree(
            "error: cannot pull with rebase: You have unstaged changes.\nerror: Please commit or stash them.",
        )
        .expect("classified");
        assert_eq!(d.operation, "pull");
        assert!(d.files.is_empty());

        let d =
            parse_dirty_worktree("error: cannot rebase: Your index contains uncommitted changes.")
                .expect("classified");
        assert_eq!(d.operation, "rebase");
        assert!(d.files.is_empty());
    }

    #[test]
    fn unrelated_stderr_is_none() {
        assert!(parse_dirty_worktree("fatal: couldn't find remote ref dev").is_none());
        assert!(parse_dirty_worktree("").is_none());
        assert!(parse_dirty_worktree(
            "error: Your local changes to the following files would be overwritten by merge:\nPlease commit"
        )
        .is_none());
        // Not a refusal we translate: keep raw stderr.
        assert!(
            parse_dirty_worktree("error: pathspec 'x' did not match any file(s) known to git")
                .is_none()
        );
    }
}
