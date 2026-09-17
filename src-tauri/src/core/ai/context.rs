//! 上下文构建（P11，ADR-013）：数据采集器 + 采样 + 隐私过滤。
//!
//! - **隐私红线**：路径先过 [`PathFilter`]（glob 规则，大小写不敏感、
//!   宁多勿漏），命中的文件**不进入**上下文（diff 文本与 numstat 统计都剔除）；
//! - **智能提交消息**：staged DiffModel → 按变更量加权采样，受每文件行数
//!   与总字符预算约束，超预算文件降级为占位行；
//! - **日报/周报**：log+numstat 采集结果 → 紧凑文本（只含 message/author/
//!   日期/numstat，默认不发 diff）；大范围自动分批（map-reduce 前置）。

use crate::core::engine::{DiffLineKind, DiffModel, NumstatCommit};
use regex::Regex;

// =====================
// 隐私路径过滤
// =====================

/// glob 排除规则编译后的过滤器。
///
/// 匹配语义：`*` 跨目录段、`?` 单字符；对**完整路径**与**文件名**各匹配一次，
/// 因此 `*.env` 命中 `config/.env`（文件名），`*secret*` 命中任意层目录；
/// 大小写不敏感（隐私方向宁可多排除）。
pub struct PathFilter {
    regexes: Vec<Regex>,
    /// 原始规则（调试/展示）。
    pub patterns: Vec<String>,
}

impl PathFilter {
    pub fn new(patterns: &[String]) -> Self {
        let regexes = patterns
            .iter()
            .filter(|p| !p.trim().is_empty())
            .map(|p| {
                let mut re = String::from("(?i)^");
                for c in p.trim().chars() {
                    match c {
                        '*' => re.push_str(".*"),
                        '?' => re.push('.'),
                        _ => re.push_str(&regex::escape(&c.to_string())),
                    }
                }
                re.push('$');
                Regex::new(&re).ok()
            })
            .collect::<Vec<Option<Regex>>>()
            .into_iter()
            .flatten()
            .collect();
        Self {
            regexes,
            patterns: patterns.to_vec(),
        }
    }

    /// 无规则过滤器（全部放行）。
    pub fn empty() -> Self {
        Self::new(&[])
    }

    pub fn is_empty(&self) -> bool {
        self.regexes.is_empty()
    }

    pub fn is_excluded(&self, path: &str) -> bool {
        let basename = path.rsplit(['/', '\\']).next().unwrap_or(path);
        self.regexes
            .iter()
            .any(|re| re.is_match(path) || re.is_match(basename))
    }

    /// 过滤一批路径，返回 (保留, 排除数)。
    pub fn filter<'a, I: IntoIterator<Item = &'a str>>(&self, paths: I) -> (Vec<String>, usize) {
        let mut kept = Vec::new();
        let mut excluded = 0usize;
        for p in paths {
            if self.is_excluded(p) {
                excluded += 1;
            } else {
                kept.push(p.to_string());
            }
        }
        (kept, excluded)
    }
}

// =====================
// 提交消息：staged diff 采样
// =====================

/// 采样预算（PLAN P11：大 diff 截断策略）。
#[derive(Debug, Clone, Copy)]
pub struct DiffBudget {
    /// 单文件最多进入上下文的变更/上下文行数。
    pub per_file_lines: usize,
    /// 全部文件合计的字符预算（≈ token 的 3~4 倍）。
    pub total_chars: usize,
}

impl Default for DiffBudget {
    fn default() -> Self {
        Self {
            per_file_lines: 300,
            total_chars: 60_000,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DiffFileCtx {
    pub path: String,
    pub is_binary: bool,
    pub added: u32,
    pub removed: u32,
    /// 渲染出的 diff 片段（采样后；超预算文件为占位说明）。
    pub text: String,
    /// 该文件被截断（行数或总预算）。
    pub truncated: bool,
}

#[derive(Debug, Clone, Default)]
pub struct DiffContext {
    pub files: Vec<DiffFileCtx>,
    pub excluded_files: usize,
    /// 实际渲染的总字符数（含占位行）。
    pub total_chars: usize,
    /// 至少一个文件被截断或因预算省略。
    pub truncated: bool,
}

/// 从 staged DiffModel 构建采样上下文。
///
/// 截断策略（按文件重要性采样）：先按每文件变更行数（+|−）降序分配预算，
/// 变更量大的文件优先拿到完整片段（受 `per_file_lines` 上限）；预算耗尽后
/// 剩余文件降级为一行占位（保留路径与规模，模型仍知道它存在）。
/// 输出保持 git 的原始文件顺序。
pub fn build_diff_context(
    model: &DiffModel,
    filter: &PathFilter,
    budget: &DiffBudget,
) -> DiffContext {
    // 1. 过滤 + 初渲染（每文件独立，先不计总预算）。
    let mut rendered: Vec<Option<DiffFileCtx>> = Vec::with_capacity(model.files.len());
    let mut excluded_files = 0usize;
    for f in &model.files {
        let path = f
            .new_path
            .as_deref()
            .or(f.old_path.as_deref())
            .unwrap_or("(unknown)");
        if filter.is_excluded(path) {
            excluded_files += 1;
            rendered.push(None);
            continue;
        }
        rendered.push(Some(render_file(f, path, budget.per_file_lines)));
    }

    // 2. 按变更量降序分配总预算。
    let mut order: Vec<usize> = (0..rendered.len())
        .filter(|i| rendered[*i].is_some())
        .collect();
    order.sort_by_key(|i| {
        std::cmp::Reverse(
            rendered[*i]
                .as_ref()
                .map(|f| f.added + f.removed)
                .unwrap_or(0),
        )
    });
    let mut used = 0usize;
    let mut omitted: Vec<usize> = Vec::new();
    for &i in &order {
        let ctx = rendered[i].as_ref().expect("filtered above");
        if used + ctx.text.chars().count() <= budget.total_chars {
            used += ctx.text.chars().count();
        } else {
            omitted.push(i);
        }
    }
    for &i in &omitted {
        let ctx = rendered[i].as_mut().expect("kept above");
        ctx.text = format!(
            "[{} omitted: +{} -{} lines exceeded the context budget]\n",
            ctx.path, ctx.added, ctx.removed
        );
    }

    // 3. 汇总（保持 git 原始顺序）。
    let files: Vec<DiffFileCtx> = rendered.into_iter().flatten().collect();
    let total_chars = files.iter().map(|f| f.text.chars().count()).sum();
    let truncated = !omitted.is_empty() || files.iter().any(|f| f.truncated);
    DiffContext {
        files,
        excluded_files,
        total_chars,
        truncated,
    }
}

/// 渲染单个文件的 diff 片段（行数截断在此完成）。
fn render_file(f: &crate::core::engine::DiffFile, path: &str, max_lines: usize) -> DiffFileCtx {
    let mut added = 0u32;
    let mut removed = 0u32;
    let mut lines = 0usize;
    let mut truncated = false;
    let mut text = String::new();
    if f.binary {
        return DiffFileCtx {
            path: path.to_string(),
            is_binary: true,
            added: 0,
            removed: 0,
            text: format!("[binary file: {path}]\n"),
            truncated: false,
        };
    }
    for hunk in &f.hunks {
        // 行预算触顶：标记截断并停止（后续 hunk 丢弃）。
        if lines >= max_lines {
            truncated = true;
            break;
        }
        text.push_str(&format!(
            "@@ -{},{} +{},{} @@\n",
            hunk.old_start, hunk.old_count, hunk.new_start, hunk.new_count
        ));
        for l in &hunk.lines {
            if l.kind == DiffLineKind::Header {
                continue;
            }
            // 计数只在入样行上进行（超预算的行不计入 added/removed）。
            if lines >= max_lines {
                truncated = true;
                break;
            }
            let prefix = match l.kind {
                DiffLineKind::Add => {
                    added += 1;
                    "+"
                }
                DiffLineKind::Remove => {
                    removed += 1;
                    "-"
                }
                _ => " ",
            };
            text.push_str(prefix);
            text.push_str(&l.content);
            text.push('\n');
            lines += 1;
        }
    }
    let header = format!(
        "--- {path} (+{added} -{removed}{})\n",
        if truncated { ", truncated" } else { "" }
    );
    DiffFileCtx {
        path: path.to_string(),
        is_binary: false,
        added,
        removed,
        text: format!("{header}{text}"),
        truncated,
    }
}

/// 把采样上下文渲染为发送给模型的完整文本。
pub fn render_diff_context(ctx: &DiffContext) -> String {
    let mut out = String::new();
    let added: u32 = ctx.files.iter().map(|f| f.added).sum();
    let removed: u32 = ctx.files.iter().map(|f| f.removed).sum();
    out.push_str(&format!(
        "Staged changes: {} files (+{added} -{removed})\n",
        ctx.files.len()
    ));
    if ctx.excluded_files > 0 {
        out.push_str(&format!(
            "({} files were excluded by privacy rules and are NOT shown)\n",
            ctx.excluded_files
        ));
    }
    out.push('\n');
    for f in &ctx.files {
        out.push_str(&f.text);
    }
    out
}

// =====================
// 日报/周报：log+numstat 上下文
// =====================

/// map-reduce 触发阈值（PLAN P11：周报 300+ 提交分批摘要再汇总）。
pub const MAP_REDUCE_COMMIT_THRESHOLD: usize = 120;
pub const MAP_REDUCE_CHARS_THRESHOLD: usize = 24_000;
/// map 阶段单批上限。
pub const BATCH_MAX_COMMITS: usize = 60;
pub const BATCH_MAX_CHARS: usize = 12_000;

/// 一条提交的紧凑渲染（只含 message/author/日期/numstat —— 不发 diff）。
fn render_commit(c: &NumstatCommit, filter: &PathFilter, out: &mut String) -> usize {
    let before = out.len();
    out.push_str(&format!(
        "- {} {} <{}> {}\n  {}\n",
        c.date, c.short_hash, c.email, c.author, c.subject
    ));
    let stats: Vec<String> = c
        .files
        .iter()
        .filter(|f| !filter.is_excluded(&f.path))
        .map(|f| {
            if f.binary {
                format!("{} (binary)", f.path)
            } else {
                format!("{} +{} -{}", f.path, f.additions, f.deletions)
            }
        })
        .collect();
    if !stats.is_empty() {
        out.push_str("  ");
        out.push_str(&stats.join("; "));
        out.push('\n');
    }
    out.len() - before
}

/// 渲染一批提交，返回 (文本, 排除文件数)。拥有切片版本。
pub fn render_report_batch(commits: &[NumstatCommit], filter: &PathFilter) -> (String, usize) {
    let refs: Vec<&NumstatCommit> = commits.iter().collect();
    render_report_batch_refs(&refs, filter)
}

/// 引用切片版本（map-reduce 分批用，避免克隆）。
pub fn render_report_batch_refs(
    commits: &[&NumstatCommit],
    filter: &PathFilter,
) -> (String, usize) {
    let mut out = String::new();
    let mut excluded = 0usize;
    for c in commits {
        let _ = render_commit(c, filter, &mut out);
        excluded += c
            .files
            .iter()
            .filter(|f| filter.is_excluded(&f.path))
            .count();
    }
    (out, excluded)
}

/// 大范围分批：按提交数与渲染字符双上限切分（顺序保持，时间近邻成批）。
pub fn batch_for_map_reduce(
    commits: &[NumstatCommit],
    filter: &PathFilter,
    max_commits: usize,
    max_chars: usize,
) -> Vec<Vec<usize>> {
    let mut batches = Vec::new();
    let mut current: Vec<usize> = Vec::new();
    let mut chars = 0usize;
    for (i, c) in commits.iter().enumerate() {
        let mut probe = String::new();
        let size = render_commit(c, filter, &mut probe);
        if !current.is_empty() && (current.len() >= max_commits || chars + size > max_chars) {
            batches.push(std::mem::take(&mut current));
            chars = 0;
        }
        current.push(i);
        chars += size;
    }
    if !current.is_empty() {
        batches.push(current);
    }
    batches
}

/// 是否需要 map-reduce（提交数或渲染体量超阈值）。
pub fn needs_map_reduce(commits: &[NumstatCommit], filter: &PathFilter) -> bool {
    commits.len() > MAP_REDUCE_COMMIT_THRESHOLD || {
        let (text, _) = render_report_batch(commits, filter);
        text.len() > MAP_REDUCE_CHARS_THRESHOLD
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::engine::{DiffFile, DiffHunk, DiffLine, DiffLineKind, NumstatFile};

    fn patterns(list: &[&str]) -> PathFilter {
        PathFilter::new(&list.iter().map(|s| s.to_string()).collect::<Vec<_>>())
    }

    fn model(files: Vec<DiffFile>) -> DiffModel {
        DiffModel {
            id: 0,
            source: crate::core::engine::DiffSource::Staged,
            old_revision: None,
            new_revision: None,
            files,
        }
    }

    fn file(path: &str, adds: &[&str], dels: &[&str]) -> DiffFile {
        let mut lines = Vec::new();
        for d in dels {
            lines.push(DiffLine {
                content: d.to_string(),
                left_no: Some(1),
                right_no: None,
                kind: DiffLineKind::Remove,
                no_eol: false,
            });
        }
        for a in adds {
            lines.push(DiffLine {
                content: a.to_string(),
                left_no: None,
                right_no: Some(1),
                kind: DiffLineKind::Add,
                no_eol: false,
            });
        }
        DiffFile {
            old_path: Some(path.to_string()),
            new_path: Some(path.to_string()),
            similarity: None,
            binary: false,
            hunks: vec![DiffHunk {
                old_start: 1,
                old_count: dels.len() as u32,
                new_start: 1,
                new_count: adds.len() as u32,
                header: String::new(),
                lines,
            }],
        }
    }

    fn numstat(path: &str, adds: u32, dels: u32) -> NumstatFile {
        NumstatFile {
            path: path.into(),
            orig_path: None,
            additions: adds,
            deletions: dels,
            binary: false,
        }
    }

    // ---------- PathFilter ----------

    #[test]
    fn glob_matches_full_path_basename_and_case_insensitive() {
        let f = patterns(&["*.env", "*.env.*", "*secret*", "*.pem"]);
        assert!(f.is_excluded("config/.env"));
        assert!(!f.is_excluded("app/environment.rs"));
        assert!(f.is_excluded("deploy/.env.production"));
        assert!(f.is_excluded("src/SECRET_KEY.txt"));
        assert!(f.is_excluded("server.pem"));
        assert!(!f.is_excluded("src/main.rs"));
    }

    #[test]
    fn filter_counts_exclusions() {
        let f = patterns(&["*.env", "*.env.*"]);
        let (kept, excluded) = f.filter(["src/main.rs", ".env", "x/.env.local"]);
        assert_eq!(kept, vec!["src/main.rs".to_string()]);
        assert_eq!(excluded, 2);
    }

    // ---------- diff 采样 ----------

    #[test]
    fn diff_context_excludes_and_counts() {
        let m = model(vec![
            file("src/main.rs", &["fn main() {}"], &["old"]),
            file(".env", &["SECRET=1"], &[]),
        ]);
        let ctx = build_diff_context(&m, &patterns(&["*.env"]), &DiffBudget::default());
        assert_eq!(ctx.files.len(), 1);
        assert_eq!(ctx.excluded_files, 1);
        assert!(ctx.files[0].text.contains("src/main.rs"));
        assert!(!render_diff_context(&ctx).contains("SECRET"));
    }

    #[test]
    fn diff_context_budget_drops_smallest_files_to_placeholders() {
        // 大文件渲染体量超过总预算 → 降级为占位，小文件保留。
        let big: Vec<String> = (0..2000).map(|i| format!("line{i}")).collect();
        let big_refs: Vec<&str> = big.iter().map(String::as_str).collect();
        let m = model(vec![
            file("small.txt", &["tiny"], &[]),
            file("big.txt", &big_refs, &[]),
        ]);
        let ctx = build_diff_context(
            &m,
            &PathFilter::empty(),
            &DiffBudget {
                per_file_lines: 2000,
                total_chars: 8_000,
            },
        );
        assert_eq!(ctx.files.len(), 2);
        assert!(ctx.truncated);
        let small = ctx.files.iter().find(|f| f.path == "small.txt").unwrap();
        let bigf = ctx.files.iter().find(|f| f.path == "big.txt").unwrap();
        // 大文件独占超预算 → 整体降级为占位；小文件保留。
        assert!(bigf.text.contains("omitted"), "{}", bigf.text);
        assert!(small.text.contains("tiny"), "{}", small.text);
        assert!(ctx.total_chars < 12_000);
    }

    #[test]
    fn diff_context_per_file_line_cap() {
        let lines: Vec<String> = (0..500).map(|i| format!("l{i}")).collect();
        let refs: Vec<&str> = lines.iter().map(String::as_str).collect();
        let m = model(vec![file("a.txt", &refs, &[])]);
        let ctx = build_diff_context(
            &m,
            &PathFilter::empty(),
            &DiffBudget {
                per_file_lines: 50,
                total_chars: 100_000,
            },
        );
        let f = &ctx.files[0];
        assert!(f.truncated);
        assert!(f.text.contains("truncated"));
        assert_eq!(f.added, 50, "counts only sampled lines");
    }

    #[test]
    fn binary_file_gets_placeholder() {
        let mut f = file("img.png", &[], &[]);
        f.binary = true;
        f.hunks.clear();
        let ctx = build_diff_context(
            &model(vec![f]),
            &PathFilter::empty(),
            &DiffBudget::default(),
        );
        assert!(ctx.files[0].is_binary);
        assert!(ctx.files[0].text.contains("binary"));
    }

    #[test]
    fn render_includes_privacy_note_and_totals() {
        let m = model(vec![file("src/a.rs", &["x"], &["y"])]);
        let ctx = build_diff_context(&m, &patterns(&["*.env"]), &DiffBudget::default());
        let text = render_diff_context(&ctx);
        assert!(text.contains("Staged changes: 1 files"));
        assert!(text.contains("--- src/a.rs"));
        assert!(text.contains("+1 -1"));
    }

    // ---------- 报告上下文 ----------

    fn commit(subject: &str, files: Vec<NumstatFile>) -> NumstatCommit {
        NumstatCommit {
            hash: "0123456789abcdef0123456789abcdef01234567".into(),
            short_hash: "0123456".into(),
            author: "Alice".into(),
            email: "alice@example.com".into(),
            date: "2026-09-17T09:00:00+08:00".into(),
            subject: subject.into(),
            repo: "repo-a".into(),
            files,
        }
    }

    #[test]
    fn report_batch_renders_compact_and_excludes_paths() {
        let commits = vec![
            commit(
                "feat: add login",
                vec![numstat("src/login.ts", 120, 4), numstat(".env", 3, 0)],
            ),
            commit("fix: crash", vec![]),
        ];
        let (text, excluded) = render_report_batch(&commits, &patterns(&["*.env"]));
        assert_eq!(excluded, 1);
        assert!(text.contains("feat: add login"));
        assert!(text.contains("src/login.ts +120 -4"));
        assert!(!text.contains(".env"), "excluded path must not leak");
        assert!(text.contains("fix: crash"));
        // 上下文不含 diff 内容（默认不发 diff）。
        assert!(!text.contains("@@"));
    }

    #[test]
    fn batching_respects_commit_and_char_limits() {
        let commits: Vec<NumstatCommit> = (0..250)
            .map(|i| {
                commit(
                    &format!("commit {i}"),
                    vec![numstat("src/a.rs", 10, 1), numstat("src/b.rs", 5, 2)],
                )
            })
            .collect();
        let batches = batch_for_map_reduce(&commits, &PathFilter::empty(), 60, 12_000);
        assert_eq!(batches.len(), 5, "250/60 → ceil");
        assert!(batches.iter().all(|b| b.len() <= 60));
        // 覆盖全部且不重复。
        let all: Vec<usize> = batches.concat();
        assert_eq!(all.len(), 250);
        assert_eq!(
            all.iter()
                .copied()
                .collect::<std::collections::HashSet<_>>()
                .len(),
            250
        );
        // 单批渲染体积不超上限（除非单条超限）。
        for b in &batches {
            let (text, _) = render_report_batch_refs(
                &b.iter().map(|&i| &commits[i]).collect::<Vec<_>>(),
                &PathFilter::empty(),
            );
            assert!(text.len() <= 14_000, "{}", text.len());
        }
    }

    #[test]
    fn map_reduce_threshold() {
        let small: Vec<NumstatCommit> = (0..10)
            .map(|i| commit(&format!("c{i}"), vec![numstat("a.rs", 1, 0)]))
            .collect();
        assert!(!needs_map_reduce(&small, &PathFilter::empty()));
        let big: Vec<NumstatCommit> = (0..130)
            .map(|i| commit(&format!("c{i}"), vec![numstat("a.rs", 1, 0)]))
            .collect();
        assert!(needs_map_reduce(&big, &PathFilter::empty()));
    }
}
