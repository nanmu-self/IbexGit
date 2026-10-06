/**
 * 前端路径拼接工具。
 *
 * 存在的理由：两个对话框（CloneDialog / NewRepoDialog）都要把「父目录 +
 * 名称」拼成最终路径，判据必须一致 —— 否则同一份路径在两处会拼出不同
 * 形态的结果。
 *
 * 判据读的是**路径自身的形态**，不是运行时平台。早先 NewRepoDialog 用
 * `navigator.userAgent.includes("Windows")` 反推平台，隐含假设「用户填的
 * 路径风格 = 运行平台」，并在 `C:/repos`（盘符 + 正斜杠）这类形态上给出
 * 与用户直觉不符的混合斜杠。平台判据还把代码绑死在 WebView 的 UA 默认值
 * 上 —— 那是外部依赖，静默退化时无人察觉。
 */

/**
 * 去掉路径尾部的斜杠（`D:\repos\` → `D:\repos`）。
 *
 * 不归一的话，父目录带不带尾斜杠会拼出语义不同的两条路径：git 见到
 * `D:\repos\` 会克隆到 `D:\repos\<repo>`，见到 `D:\repos` 则把 repos
 * 本身当仓库根 —— 同一个界面、同一份意图，两种结果。
 */
export function trimTrailingSep(dir: string): string {
  return dir.replace(/[\\/]+$/, "");
}

/**
 * 按路径自身的形态选分隔符：含反斜杠（UNC `\\wsl$\...`、扩展长度前缀
 * `\\?\C:\...`）或以盘符开头（`C:`、`d:`）→ 反斜杠，否则正斜杠。
 *
 * 纯字符串函数，不读 navigator —— 可在 Node/CI 里直接断言。
 */
export function sepFor(dir: string): string {
  return dir.includes("\\") || /^[A-Za-z]:/.test(dir) ? "\\" : "/";
}

/**
 * 父目录 + 名称 → 最终路径。父目录尾斜杠已归一，空段不产生多余分隔符。
 *
 * 注意：波浪号（`~/x`）不展开，仍按字面拼 —— 文件夹选择器返回的是绝对
 * 路径，手输 `~` 本就不是有效输入。
 */
export function joinPath(dir: string, name: string): string {
  const base = trimTrailingSep(dir.trim());
  const leaf = name.trim();
  if (!base) return leaf;
  if (!leaf) return base;
  return base + sepFor(base) + leaf;
}

/**
 * Win32 保留设备名（不区分大小写，扩展名形式同样命中：`CON.txt`）。
 * 这些名字在文件系统里不可创建，但`create_dir_all` 未必报错——
 * 静默失败比报错更难排查。
 */
const WIN32_RESERVED = new Set([
  "con", "prn", "aux", "nul",
  "com1", "com2", "com3", "com4", "com5", "com6", "com7", "com8", "com9",
  "lpt1", "lpt2", "lpt3", "lpt4", "lpt5", "lpt6", "lpt7", "lpt8", "lpt9",
]);

/**
 * 仓库名是否可安全用作路径末段。CloneDialog 与 NewRepoDialog 共用 ——
 * 名字会真的变成目录层级，漏校验就会静默建出嵌套目录（回显与落点不一致，
 * 正是ADR-015 要消灭的那类"同一意图两种结果"）。
 *
 * 这是 **UI 便利校验，不是安全边界**：真正的守门在后端
 * （存在性/非空守卫见 `core/engine/cli.rs::clone_repo`）。
 *
 * 覆盖的规则（每条都对应一个真实可见的失败形态）：
 * - 空 /纯空白 → 无效
 * - 含 `\` 或 `/` → `create_dir_all` 会静默建出嵌套目录
 * - 全点号（`.` / `..`）→ 目录穿越
 * - 含 `:` → `C:foo` 这类盘符相对名，语义随当前盘符漂移
 * - Win32 保留名（含 `CON.txt` 这类带扩展名的形式）
 * - 尾点 → Win32 静默剥离，回显 `x.` 而落点是 `x`
 *
 * 职责边界：前后空白与尾空格由 `joinPath` 的 `trim` 消除（打字噪声，
 * 落点与干净名字一致），故不在此拦。
 */
export function isSafeRepoName(raw: string): boolean {
  // 全部检查统一基于 trim 后的名字：joinPath 拼接时也会 trim，
  // 校验基准必须与实际落点同源（否则 `"  myrepo  "` 这类纯空格噪声
  // 会被误判为非法）。
  const name = raw.trim();
  if (!name) return false;
  if (/[\\/]/.test(name)) return false;
  // 尾点：Win32 静默剥离 → 回显 `x.` 而落点是 `x`。
  //（尾空格已被上面的 trim 消除，不构成落点偏移。）
  if (name.endsWith(".")) return false;
  // `C:foo` 这类盘符相对名：语义随当前盘符漂移。
  if (name.includes(":")) return false;
  // 全点号：`.` / `..` 是目录穿越，不是新目录名。
  if (/^\.+$/.test(name)) return false;
  // 保留名带扩展名同样命中（`CON.txt` =设备 CON）。
  const stem = name.split(".")[0] ?? "";
  if (WIN32_RESERVED.has(stem.toLowerCase())) return false;
  return true;
}