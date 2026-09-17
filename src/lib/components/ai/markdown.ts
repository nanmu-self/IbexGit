/**
 * 轻量 Markdown 渲染器（P11 日报/周报）。
 *
 * AI 报告只用到 Markdown 的一个小子集：标题、有序/无序列表、粗体/斜体/
 * 行内代码、围栏代码块、引用、分隔线、表格、段落。渲染流程：
 * 1. 先对整篇源文本做 HTML 转义（不可信内容安全——流式期间渲染的是
 *    未完成文本，转义先行保证任何中间状态都不产生未转义的插值）；
 * 2. 再逐行解析出上述结构，输出的标签全部由本模块生成。
 *
 * 不引入第三方 markdown 库（ADR-010 的包体纪律；报告场景足够）。
 */

function escapeHtml(s: string): string {
  return s
    .replaceAll("&", "&amp;")
    .replaceAll("<", "&lt;")
    .replaceAll(">", "&gt;")
    .replaceAll('"', "&quot;")
    .replaceAll("'", "&#39;");
}

/** 行内格式：`code`、**bold**、*italic*（输入已转义）。 */
function inline(s: string): string {
  return s
    .replace(/`([^`]+)`/g, '<code class="rounded bg-muted px-1 py-0.5 font-mono text-[11px]">$1</code>')
    .replace(/\*\*([^*]+)\*\*/g, "<strong>$1</strong>")
    .replace(/(^|[^*])\*([^*\s][^*]*)\*/g, "$1<em>$2</em>");
}

export function renderMarkdown(src: string): string {
  const lines = escapeHtml(src).split(/\r?\n/);
  const out: string[] = [];
  let inCode = false;
  let codeBuf: string[] = [];
  let listType: "ul" | "ol" | null = null;
  let paraBuf: string[] = [];
  let tableBuf: string[][] = [];

  const closeList = () => {
    if (listType) {
      out.push(`</${listType}>`);
      listType = null;
    }
  };
  const closePara = () => {
    if (paraBuf.length) {
      out.push(`<p class="my-1.5 leading-relaxed">${inline(paraBuf.join("<br>"))}</p>`);
      paraBuf = [];
    }
  };
  const closeTable = () => {
    if (tableBuf.length) {
      const [head, ...rows] = tableBuf;
      const cell = (c: string, tag: string) =>
        `<${tag} class="border border-border px-2 py-1 text-left">${inline(c.trim())}</${tag}>`;
      const thead = `<thead><tr>${head.map((c) => cell(c, "th")).join("")}</tr></thead>`;
      const tbody = `<tbody>${rows
        .map((r) => `<tr>${r.map((c) => cell(c, "td")).join("")}</tr>`)
        .join("")}</tbody>`;
      out.push(`<div class="my-2 overflow-x-auto"><table class="w-full text-[12px]">${thead}${tbody}</table></div>`);
      tableBuf = [];
    }
  };
  const isTableDivider = (l: string) =>
    /^\s*\|?[\s:|-]+\|[\s:|]*$/.test(l) && l.includes("-");

  for (const raw of lines) {
    // ---- 围栏代码块 ----
    if (raw.trimStart().startsWith("```")) {
      if (inCode) {
        out.push(
          `<pre class="my-2 overflow-x-auto rounded-md border bg-muted/40 p-2 font-mono text-[11px]">${codeBuf.join("\n")}</pre>`,
        );
        codeBuf = [];
        inCode = false;
      } else {
        closePara();
        closeList();
        closeTable();
        inCode = true;
      }
      continue;
    }
    if (inCode) {
      codeBuf.push(raw);
      continue;
    }

    const line = raw.trimEnd();

    // ---- 表格 ----
    if (line.trim().startsWith("|") && line.trim().endsWith("|")) {
      if (isTableDivider(line)) continue; // 对齐行
      closePara();
      closeList();
      tableBuf.push(
        line
          .trim()
          .replace(/^\|/, "")
          .replace(/\|$/, "")
          .split("|"),
      );
      continue;
    }
    closeTable();

    // ---- 标题 ----
    const heading = /^(#{1,6})\s+(.*)$/.exec(line);
    if (heading) {
      closePara();
      closeList();
      const level = heading[1].length;
      const cls =
        level <= 2
          ? "mt-3 mb-1.5 text-[15px] font-semibold"
          : "mt-2.5 mb-1 text-[13px] font-semibold";
      out.push(`<h${level} class="${cls}">${inline(heading[2])}</h${level}>`);
      continue;
    }

    // ---- 分隔线 ----
    if (/^\s*(-{3,}|\*{3,})\s*$/.test(line)) {
      closePara();
      closeList();
      out.push('<hr class="my-3 border-border">');
      continue;
    }

    // ---- 引用 ----
    const quote = /^&gt;\s?(.*)$/.exec(line);
    if (quote) {
      closePara();
      closeList();
      out.push(
        `<blockquote class="my-1.5 border-l-2 border-border pl-2.5 text-muted-foreground">${inline(quote[1])}</blockquote>`,
      );
      continue;
    }

    // ---- 列表 ----
    const ul = /^[-*+]\s+(.*)$/.exec(line.trim());
    const ol = /^(\d+)[.)]\s+(.*)$/.exec(line.trim());
    if (ul) {
      closePara();
      if (listType !== "ul") {
        closeList();
        out.push('<ul class="my-1.5 list-disc space-y-1 pl-5">');
        listType = "ul";
      }
      out.push(`<li>${inline(ul[1])}</li>`);
      continue;
    }
    if (ol) {
      closePara();
      if (listType !== "ol") {
        closeList();
        out.push('<ol class="my-1.5 list-decimal space-y-1 pl-5">');
        listType = "ol";
      }
      out.push(`<li>${inline(ol[2])}</li>`);
      continue;
    }

    // ---- 空行 / 段落 ----
    if (line.trim() === "") {
      closeList();
      closePara();
      continue;
    }
    closeList();
    paraBuf.push(line);
  }

  // 收尾
  if (inCode && codeBuf.length) {
    out.push(
      `<pre class="my-2 overflow-x-auto rounded-md border bg-muted/40 p-2 font-mono text-[11px]">${codeBuf.join("\n")}</pre>`,
    );
  }
  closeTable();
  closeList();
  closePara();
  return out.join("\n");
}
