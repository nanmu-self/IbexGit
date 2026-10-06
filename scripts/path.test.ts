/**
 * `src/lib/path.ts` 的断言。纯函数、无浏览器依赖：
 *
 *   node --experimental-strip-types scripts/path.test.ts
 *
 * 放在 `scripts/` 而非 `src/`：Node 的类型剥离要求显式 `.ts` 扩展名，
 * 而前端 tsconfig 未开 `allowImportingTsExtensions`，放src/ 会让
 * svelte-check 报错（两种工具链对import 写法的要求互斥）。
 * 本文件通过 .gitignore 例外（`!scripts/path.test.ts`）入库并接入 CI：
 * 断言不入库则新 clone 上不存在，source of truth 无强制力。
 * 同目录的 `_*.py` 是本地探针，仍不入库。
 *
 * 路径拼接是 ADR-015 里的 source of truth，规则变更必须先改这里的断言。
 * 注意：不要用正则剥 TS 标注再跑（贪婪匹配会吃掉整个文件）。
 */
const { trimTrailingSep, sepFor, joinPath, isSafeRepoName } = (await import(
  "../src/lib/path.ts"
)) as typeof import("../src/lib/path");

let failed = 0;
let passed = 0;

function eq(actual: unknown, expected: unknown, label: string): void {
  const a = JSON.stringify(actual);
  const e = JSON.stringify(expected);
  if (a === e) {
    passed++;
  } else {
    failed++;
    console.error(`FAIL ${label}\n  期望 ${e}\n  实际 ${a}`);
  }
}

// ---------- trimTrailingSep ----------
eq(trimTrailingSep("C:\\repos\\"), "C:\\repos", "尾斜杠（反）被剥");
eq(trimTrailingSep("C:\\repos\\\\"), "C:\\repos", "多尾斜杠全剥");
eq(trimTrailingSep("/mnt/c/repos/"), "/mnt/c/repos", "尾斜杠（正）被剥");
eq(trimTrailingSep("D:\\a\\b"), "D:\\a\\b", "中间斜杠不动");
eq(trimTrailingSep("C:repos"), "C:repos", "盘符相对名不动");

// ---------- sepFor ----------
eq(sepFor("C:\\repos"), "\\", "含反斜杠 → 反斜杠");
eq(sepFor("C:/repos"), "\\", "盘符+正斜杠 → 反斜杠");
eq(sepFor("C:repos"), "\\", "盘符相对名 → 反斜杠");
eq(sepFor("\\\\wsl$\\Ubuntu"), "\\", "UNC → 反斜杠");
eq(sepFor("\\\\?\\C:\\repos"), "\\", "扩展长度前缀 → 反斜杠");
eq(sepFor("/mnt/c"), "/", "POSIX → 正斜杠");
eq(sepFor("relative/path"), "/", "相对 POSIX → 正斜杠");

// ---------- joinPath ----------
eq(joinPath("C:\\repos", "myrepo"), "C:\\repos\\myrepo", "常规 Windows");
eq(joinPath("C:\\repos\\", "myrepo"), "C:\\repos\\myrepo", "父目录带尾斜杠归一");
eq(joinPath("  C:\\repos  ", "  myrepo  "), "C:\\repos\\myrepo", "两端空白 trim");
eq(joinPath("/mnt/c/repos", "p"), "/mnt/c/repos/p", "POSIX 父目录");
eq(joinPath("", "p"), "p", "空父目录 → 只返名称");
eq(joinPath("C:\\repos", ""), "C:\\repos", "空名称 → 只返父目录");

// 归一不变量：同义输入必须产出同一路径（ADR-015 §5.1 的核心承诺）
const g = [
  joinPath("C:\\repos", "p"),
  joinPath("C:\\repos\\", "p"),
  joinPath("  C:\\repos  ", "  p  "),
];
eq(new Set(g).size, 1, "归一不变量：三种同义写法产出同一结果");

// ---------- isSafeRepoName ----------
for (const ok of ["myrepo", "my-repo", "repo_1", "a", "项目", "CSharp"]) {
  eq(isSafeRepoName(ok), true, `合法名 ${JSON.stringify(ok)}`);
}
for (const bad of [
  "a/b",
  "a\\b",
  "..",
  ".",
  "C:foo",
  "CON",
  "con",
  "NUL.txt",
  "COM1",
  "lpt9",
  "trailing.",
  "  trailing.  ",
  "",
]) {
  eq(isSafeRepoName(bad), false, `非法名 ${JSON.stringify(bad)}`);
}

// 前后空白是打字噪声：joinPath 内部会 trim，落点与干净名字一致 → 放行。
// 尾空格因此已被 joinPath 消除（不是 isSafeRepoName 的职责）；
// 尾点不同——Win32 静默剥离后落点少一个字符，必须拦。
eq(isSafeRepoName("  myrepo  "), true, "合法名带前后空白仍合法");
eq(isSafeRepoName("  a/b  "), false, "非法名带前后空白仍非法");
eq(isSafeRepoName("   "), false, "纯空白名无效");

// 前后 trim 后仍有尾点 → 必须拦（joinPath 不会剥尾点）
eq(joinPath("C:\\repos", "trailing."), "C:\\repos\\trailing.", "尾点原样拼出（由 isSafeRepoName 拦截）");

console.log(`\npath.test.ts: ${passed} passed, ${failed} failed`);
// 本脚本只由 Node 执行。经 globalThis 访问以避免依赖 @types/node。
const proc = (globalThis as { process?: { exitCode?: number } }).process;
if (proc) proc.exitCode = failed > 0 ? 1 : 0;
