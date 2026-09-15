/**
 * Diff row model (P4): flattens a DiffModel into a fixed-count list of
 * virtualizable rows for the unified and split views, with:
 * - within-hunk context collapsing (client-side expand)
 * - hunk-edge expansion zones (server refetch with `--unified=N`)
 * - word-diff pairing of adjacent remove/add runs
 */
import type { DiffFile, DiffLine, DiffHunk, DiffModel, DiffSource } from "$lib/git/bindings";
import { wordDiff, type WordSpan } from "./wordDiff";

/** Context lines kept around a collapsed run (top & bottom). */
const COLLAPSE_KEEP = 3;
/** Minimum run length worth collapsing (must exceed 2 * COLLAPSE_KEEP). */
const COLLAPSE_MIN = 10;

export type ViewMode = "unified" | "split";

/** Client-side expansion state of one collapsed context run. */
export type ExpandedRuns = Set<string>;

export interface FileCtx {
  file: DiffFile;
  index: number;
  path: string;
  /** Line-level ops allowed (source gating + file kind). */
  lineOpsAllowed: boolean;
  /** Image diff candidate by extension. */
  isImage: boolean;
}

export type Row =
  | { t: "file-header"; file: FileCtx; adds: number; dels: number }
  | { t: "binary"; file: FileCtx }
  | { t: "image"; file: FileCtx }
  | { t: "hunk-header"; file: FileCtx; hunk: DiffHunk; hunkIndex: number; canStage: boolean }
  | {
      t: "expand";
      file: FileCtx;
      hunkIndex: number;
      dir: "up" | "down";
      label: string;
    }
  | { t: "line"; file: FileCtx; hunkIndex: number; line: DiffLine; lineIndex: number }
  | {
      t: "pair";
      file: FileCtx;
      hunkIndex: number;
      left: DiffLine | null;
      right: DiffLine | null;
      leftIndex: number | null;
      rightIndex: number | null;
      wordDiff: [WordSpan[], WordSpan[]] | null;
    }
  | {
      t: "collapse";
      file: FileCtx;
      hunkIndex: number;
      runKey: string;
      hidden: number;
    };

/** Row heights (px) — must match the renderer's CSS. */
export const ROW_HEIGHT: Record<Row["t"], number> = {
  "file-header": 30,
  binary: 48,
  image: 340,
  "hunk-header": 24,
  expand: 24,
  line: 20,
  pair: 20,
  collapse: 26,
};

const IMAGE_EXTS = new Set(["png", "jpg", "jpeg", "gif", "webp", "bmp", "ico", "avif", "svg"]);

export function extOf(path: string | null): string {
  if (!path) return "";
  const i = path.lastIndexOf(".");
  if (i < 0) return "";
  return path.slice(i + 1).toLowerCase();
}

export function isImageFile(file: DiffFile): boolean {
  return IMAGE_EXTS.has(extOf(file.new_path ?? file.old_path));
}

/** Whether line-level operations apply to this file (P4 gating). */
export function lineOpsAllowedFor(file: DiffFile): boolean {
  if (file.binary) return false;
  // Renames with content changes span two paths — file ops only.
  if (file.old_path && file.new_path && file.old_path !== file.new_path) return false;
  return true;
}

export function buildFileContexts(model: DiffModel): FileCtx[] {
  return model.files.map((file, index) => ({
    file,
    index,
    path: file.new_path ?? file.old_path ?? "",
    lineOpsAllowed: lineOpsAllowedFor(file),
    isImage: isImageFile(file),
  }));
}

interface BuildOpts {
  mode: ViewMode;
  expandedRuns: ExpandedRuns;
  /** Per-file refetch state: no more context above/below to fetch. */
  exhausted: Map<string, { up: boolean; down: boolean }>;
  /** hunk.old_start > 1 means content exists above the first hunk. */
}

/** Flatten one file's hunks into rows. */
function buildFileRows(fc: FileCtx, opts: BuildOpts): Row[] {
  const { file } = fc;
  const rows: Row[] = [];

  let adds = 0;
  let dels = 0;
  for (const hunk of file.hunks) {
    for (const line of hunk.lines) {
      if (line.kind === "add") adds++;
      else if (line.kind === "remove") dels++;
    }
  }
  rows.push({ t: "file-header", file: fc, adds, dels });

  if (file.binary) {
    rows.push({ t: "binary", file: fc });
    return rows;
  }
  if (fc.isImage) {
    // Image preview row; SVG and text-representable images keep their hunks
    // below so text diffs stay visible.
    rows.push({ t: "image", file: fc });
  }
  if (file.hunks.length === 0) {
    return rows;
  }

  const exhaustedUp = opts.exhausted.get(fc.path)?.up ?? false;
  const exhaustedDown = opts.exhausted.get(fc.path)?.down ?? false;

  file.hunks.forEach((hunk, hunkIndex) => {
    // Expand-up zone: content exists above when the hunk doesn't start at
    // the top of the file.
    if (hunk.old_start > 1 && !exhaustedUp) {
      rows.push({ t: "expand", file: fc, hunkIndex, dir: "up", label: "↑" });
    }

    rows.push({
      t: "hunk-header",
      file: fc,
      hunk,
      hunkIndex,
      canStage: fc.lineOpsAllowed,
    });

    // Group lines into runs of context vs change blocks.
    const lines = hunk.lines;
    let i = 0;
    while (i < lines.length) {
      const line = lines[i];
      if (line.kind !== "context") {
        // Change block: consume the whole adjacent run.
        const start = i;
        while (i < lines.length && lines[i].kind !== "context") i++;
        emitChangeBlock(fc, hunkIndex, lines, start, i, opts.mode, rows);
        continue;
      }
      // Context run.
      const start = i;
      while (i < lines.length && lines[i].kind === "context") i++;
      const runLen = i - start;
      if (runLen > COLLAPSE_MIN) {
        const runKey = `${fc.index}:${hunkIndex}:${start}`;
        if (opts.expandedRuns.has(runKey)) {
          emitContextLines(fc, hunkIndex, lines, start, i, opts.mode, rows);
        } else {
          emitContextLines(
            fc,
            hunkIndex,
            lines,
            start,
            start + COLLAPSE_KEEP,
            opts.mode,
            rows,
          );
          rows.push({
            t: "collapse",
            file: fc,
            hunkIndex,
            runKey,
            hidden: runLen - COLLAPSE_KEEP * 2,
          });
          emitContextLines(
            fc,
            hunkIndex,
            lines,
            i - COLLAPSE_KEEP,
            i,
            opts.mode,
            rows,
          );
        }
      } else {
        emitContextLines(fc, hunkIndex, lines, start, i, opts.mode, rows);
      }
    }

    // Expand-down zone: heuristic — always offer unless the file is
    // exhausted (the refetch result decides).
    if (!exhaustedDown) {
      rows.push({ t: "expand", file: fc, hunkIndex, dir: "down", label: "↓" });
    }
  });

  return rows;
}

function emitContextLines(
  fc: FileCtx,
  hunkIndex: number,
  lines: DiffLine[],
  from: number,
  to: number,
  mode: ViewMode,
  rows: Row[],
): void {
  for (let i = from; i < to; i++) {
    if (mode === "unified") {
      rows.push({ t: "line", file: fc, hunkIndex, line: lines[i], lineIndex: i });
    } else {
      rows.push({
        t: "pair",
        file: fc,
        hunkIndex,
        left: lines[i],
        right: lines[i],
        leftIndex: i,
        rightIndex: i,
        wordDiff: null,
      });
    }
  }
}

/**
 * Emit one change block (adjacent remove/add run) for unified or split mode.
 * Split mode pairs the runs positionally (min length aligned, leftovers get
 * empty cells) and computes word-level diffs for the pairs.
 */
function emitChangeBlock(
  fc: FileCtx,
  hunkIndex: number,
  lines: DiffLine[],
  from: number,
  to: number,
  mode: ViewMode,
  rows: Row[],
): void {
  const block = lines.slice(from, to);
  const dels = block.filter((l) => l.kind === "remove");
  const adds = block.filter((l) => l.kind === "add");

  if (mode === "unified") {
    block.forEach((line, k) => {
      rows.push({ t: "line", file: fc, hunkIndex, line, lineIndex: from + k });
    });
    return;
  }

  // Split: pair dels[i] with adds[i], leftovers empty.
  const n = Math.max(dels.length, adds.length);
  for (let i = 0; i < n; i++) {
    const left = dels[i] ?? null;
    const right = adds[i] ?? null;
    let wd: [WordSpan[], WordSpan[]] | null = null;
    if (left && right) wd = wordDiff(left.content, right.content);
    rows.push({
      t: "pair",
      file: fc,
      hunkIndex,
      left,
      right,
      leftIndex: left ? block.indexOf(left) + from : null,
      rightIndex: right ? block.indexOf(right) + from : null,
      wordDiff: wd,
    });
  }
}

export function buildRows(model: DiffModel, opts: BuildOpts): Row[] {
  const rows: Row[] = [];
  for (const fc of buildFileContexts(model)) {
    rows.push(...buildFileRows(fc, opts));
  }
  return rows;
}

/** Prefix-sum offsets for variable-height virtualization. */
export function computeOffsets(rows: Row[]): number[] {
  const offsets = new Array<number>(rows.length + 1);
  offsets[0] = 0;
  for (let i = 0; i < rows.length; i++) {
    offsets[i + 1] = offsets[i] + ROW_HEIGHT[rows[i].t];
  }
  return offsets;
}

/** Index of the last row fully above `y` via binary search over offsets. */
export function findIndexAt(offsets: number[], y: number): number {
  let lo = 0;
  let hi = offsets.length - 1;
  while (lo < hi) {
    const mid = (lo + hi) >> 1;
    if (offsets[mid + 1] <= y) lo = mid + 1;
    else hi = mid;
  }
  return lo;
}
