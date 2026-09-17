//! 提示词构建（P11）：提交消息 + 日报/周报（含 map-reduce 两段）。
//! 输出语言跟随前端 locale（generate 请求携带）。

use crate::core::ai::context::DiffContext;

/// locale → 模型可读语言名。
fn language_name(lang: &str) -> &'static str {
    if lang.starts_with("zh") {
        "Simplified Chinese"
    } else {
        "English"
    }
}

// =====================
// 提交消息
// =====================

pub fn commit_message_system(conventional: bool, language: &str) -> String {
    let style = if conventional {
        "Use Conventional Commits style: `type(optional scope): summary`. \
         Choose type from feat|fix|docs|style|refactor|perf|test|build|ci|chore|revert."
    } else {
        "Write a plain descriptive summary (no type prefix)."
    };
    format!(
        "You are an expert assistant that writes git commit messages.\n\
         Rules:\n\
         - Output ONLY the commit message text. No code fences, no quotes, no explanations.\n\
         - First line: imperative-mood summary, at most 50 characters, no trailing period.\n\
         - Then one blank line, then an optional body explaining WHAT changed and WHY, \
         wrapped at 72 characters. Omit the body when the change is trivial.\n\
         - {}\n\
         - Write in {} unless the diff is clearly dominated by another language.\n\
         - Never invent files or changes that are not in the provided context.",
        style,
        language_name(language)
    )
}

pub fn commit_message_user(ctx: &DiffContext) -> String {
    format!(
        "Write one commit message for the following staged changes.\n\n{}",
        crate::core::ai::context::render_diff_context(ctx)
    )
}

// =====================
// 日报 / 周报
// =====================

pub fn report_system(language: &str) -> String {
    format!(
        "You are an assistant that writes developer work reports in Markdown.\n\
         Rules:\n\
         - Output ONLY Markdown. Start with a `## ` heading.\n\
         - Structure: a one-paragraph overview, thematic highlight sections (by feature area), \
         then a `## 统计` / `## Stats` section with commits, files changed and lines added/removed.\n\
         - Group related commits; do not list every commit verbatim when themes emerge.\n\
         - NEVER invent commits, files or numbers that are not in the data. \
         If data is thin, say so briefly instead of padding.\n\
         - Write in {}.",
        language_name(language)
    )
}

pub fn report_user(
    kind: &str,
    since: &str,
    repo_count: usize,
    author_note: &str,
    context: &str,
) -> String {
    let kind_label = if kind == "weekly" { "weekly" } else { "daily" };
    format!(
        "Write a {kind_label} report from the following commits.\n\
         Range: since {since}. Repositories: {repo_count}. {author_note}\n\
         The lines under each commit are `path +added -deleted` stats (binary files have no counts).\n\n\
         {context}"
    )
}

/// map 阶段：单批摘要。
pub fn map_system(language: &str) -> String {
    format!(
        "You are summarizing git commits for a work report. \
         Output ONLY 3-8 concise Markdown bullet points covering the main themes, \
         notable files/stats, and anything unusual. Write in {}. Do not invent facts.",
        language_name(language)
    )
}

pub fn map_user(batch_context: &str) -> String {
    format!("Summarize these commits:\n\n{batch_context}")
}

/// reduce 阶段：合并各批摘要成最终报告。
pub fn reduce_system(language: &str) -> String {
    report_system(language)
}

pub fn reduce_user(kind: &str, since: &str, repo_count: usize, summaries: &[String]) -> String {
    let kind_label = if kind == "weekly" { "weekly" } else { "daily" };
    let joined: String = summaries
        .iter()
        .enumerate()
        .map(|(i, s)| format!("### Batch {} summary\n{s}\n", i + 1))
        .collect();
    format!(
        "Merge the following batch summaries of {repo_count} repositories' commits \
         into ONE coherent {kind_label} report in Markdown. Range: since {since}. \
         Deduplicate repeated themes and keep the stats meaningful.\n\n{joined}"
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn commit_system_switches_style_and_language() {
        let conv = commit_message_system(true, "zh-CN");
        assert!(conv.contains("Conventional Commits"));
        assert!(conv.contains("Simplified Chinese"));
        let plain = commit_message_system(false, "en");
        assert!(plain.contains("no type prefix"));
        assert!(plain.contains("English"));
    }

    #[test]
    fn commit_user_embeds_context() {
        let ctx = DiffContext::default();
        let user = commit_message_user(&ctx);
        assert!(user.contains("Staged changes: 0 files"));
    }

    #[test]
    fn report_prompts_carry_constraints() {
        let s = report_system("en");
        assert!(s.contains("NEVER invent"));
        let u = report_user("weekly", "2026-09-14", 2, "Author: alice@x.com", "DATA");
        assert!(u.contains("weekly") && u.contains("2026-09-14") && u.contains("DATA"));
        let m = map_user("BATCH");
        assert!(m.contains("BATCH"));
        let r = reduce_user("daily", "today", 3, &["a".into(), "b".into()]);
        assert!(r.contains("Batch 1") && r.contains("Batch 2"));
    }
}
