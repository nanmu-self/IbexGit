/**
 * Conflict editor core (P8, ADR-010): the whole module is dynamically
 * imported the first time a text conflict opens — the startup bundle never
 * carries CodeMirror.
 *
 * The editor consumes the Rust-side ConflictModel only: block line ranges
 * come in as props and are tracked through subsequent edits by mapping the
 * boundaries through each transaction (no marker re-parsing in the editor;
 * a leftover-marker check happens again server-side on resolve).
 */
import { EditorState, StateEffect, StateField, type Transaction } from "@codemirror/state";
import {
  Decoration,
  EditorView,
  WidgetType,
  keymap,
  lineNumbers,
  type DecorationSet,
} from "@codemirror/view";
import { defaultKeymap, history, historyKeymap } from "@codemirror/commands";
import { indentUnit, LanguageDescription } from "@codemirror/language";
import { languages } from "@codemirror/language-data";

export interface ConflictBlockVm {
  start: number;
  current_start: number;
  current_end: number;
  base_start: number | null;
  base_end: number | null;
  incoming_start: number;
  incoming_end: number;
  end: number;
}

export type BlockSide = "current" | "incoming" | "both";

export interface ConflictEditorOptions {
  doc: string;
  blocks: ConflictBlockVm[];
  /** Worktree path (drives language-mode detection). */
  path: string;
  labels: { ours: string; theirs: string; both: string };
  onBlocksChanged: (blocks: ConflictBlockVm[]) => void;
  onChanged: (text: string) => void;
}

export interface ConflictEditorHandle {
  getBlocks(): ConflictBlockVm[];
  getText(): string;
  /** Remove all but the chosen side of one block. */
  accept(index: number, side: BlockSide): void;
  /** Scroll the nth block into view and put the caret on its marker line. */
  gotoBlock(index: number): void;
  focus(): void;
  destroy(): void;
}

const setBlocks = StateEffect.define<ConflictBlockVm[]>();

interface FieldValue {
  blocks: ConflictBlockVm[];
  decorations: DecorationSet;
}

const conflictField = StateField.define<FieldValue>({
  create() {
    return { blocks: [], decorations: Decoration.none };
  },
  update(value, tr) {
    let blocks = value.blocks;
    for (const e of tr.effects) {
      if (e.is(setBlocks)) blocks = e.value;
    }
    if (blocks === value.blocks && !tr.docChanged) return value;
    // Track block boundaries through edits (typing shifts lines).
    if (tr.docChanged) blocks = blocks.map((b) => mapBlock(b, tr));
    return { blocks, decorations: buildDecorations(blocks, tr.state) };
  },
  provide: (f) => EditorView.decorations.from(f, (v) => v.decorations),
});

/** Map one block's line boundaries through a transaction. */
function mapBlock(b: ConflictBlockVm, tr: Transaction): ConflictBlockVm {
  const map = (line: number): number => {
    const from = Math.min(line + 1, tr.startState.doc.lines);
    const pos = tr.startState.doc.line(from).from;
    return tr.newDoc.lineAt(tr.changes.mapPos(pos, -1)).number - 1;
  };
  return {
    start: map(b.start),
    current_start: map(b.current_start),
    current_end: map(b.current_end),
    base_start: b.base_start === null ? null : map(b.base_start),
    base_end: b.base_end === null ? null : map(b.base_end),
    incoming_start: map(b.incoming_start),
    incoming_end: map(b.incoming_end),
    end: map(b.end),
  };
}

/** Widget handle: the live editor instance, set right after construction. */
let active: { view: EditorView; labels: ConflictEditorOptions["labels"] } | null = null;

class BlockButtons extends WidgetType {
  constructor(readonly at: "start" | "end") {
    super();
  }

  override eq(other: BlockButtons): boolean {
    return this.at === other.at;
  }

  override toDOM(view: EditorView): HTMLElement {
    const wrap = document.createElement("span");
    wrap.className = "cm-conflict-actions";
    const labels = active?.labels ?? { ours: "ours", theirs: "theirs", both: "both" };
    const mk = (label: string, side: BlockSide, cls: string) => {
      const btn = document.createElement("button");
      btn.type = "button";
      btn.textContent = label;
      btn.className = `cm-conflict-btn ${cls}`;
      btn.onclick = (ev) => {
        ev.preventDefault();
        ev.stopPropagation();
        const pos = view.posAtDOM(wrap);
        const line = view.state.doc.lineAt(pos).number - 1;
        const blocks = view.state.field(conflictField).blocks;
        const idx = blocks.findIndex((b) =>
          this.at === "start" ? b.start === line : b.end === line,
        );
        if (idx >= 0) activeHandle?.accept(idx, side);
      };
      return btn;
    };
    if (this.at === "start") {
      wrap.append(mk(labels.ours, "current", "cm-btn-current"), mk(labels.both, "both", ""));
    } else {
      wrap.append(mk(labels.theirs, "incoming", "cm-btn-incoming"));
    }
    return wrap;
  }

  override ignoreEvent(): boolean {
    return false;
  }
}

let activeHandle: ConflictEditorHandle | null = null;

const oursLine = () => Decoration.line({ class: "cm-conflict-ours" });
const baseLine = () => Decoration.line({ class: "cm-conflict-base" });
const theirsLine = () => Decoration.line({ class: "cm-conflict-theirs" });

function buildDecorations(blocks: ConflictBlockVm[], state: EditorState): DecorationSet {
  const ranges: { from: number; to: number; deco: Decoration }[] = [];
  const maxLine = state.doc.lines;
  const push = (from: number, to: number, deco: Decoration): void => {
    if (from >= maxLine) return;
    ranges.push({ from, to, deco });
  };
  for (const blk of blocks) {
    push(blk.start, blk.start, oursLine());
    push(
      blk.start,
      blk.start,
      Decoration.widget({ widget: new BlockButtons("start"), side: -1 }),
    );
    if (blk.end !== blk.start) {
      push(blk.end, blk.end, theirsLine());
      push(
        blk.end,
        blk.end,
        Decoration.widget({ widget: new BlockButtons("end"), side: 1 }),
      );
    }
    for (let i = blk.current_start; i < blk.current_end; i++) push(i, i, oursLine());
    if (blk.base_start !== null && blk.base_end !== null) {
      for (let i = blk.base_start; i < blk.base_end; i++) push(i, i, baseLine());
    }
    for (let i = blk.incoming_start; i < blk.incoming_end; i++) push(i, i, theirsLine());
  }
  return Decoration.set(
    ranges.map((r) => r.deco.range(r.from, r.to)),
    true,
  );
}

export async function createConflictEditor(
  host: HTMLElement,
  opts: ConflictEditorOptions,
): Promise<ConflictEditorHandle> {
  const languageExtension = await detectLanguage(opts.path);

  const view = new EditorView({
    state: EditorState.create({
      doc: opts.doc,
      extensions: [
        lineNumbers(),
        history(),
        keymap.of([...historyKeymap, ...defaultKeymap]),
        indentUnit.of("    "),
        conflictField,
        EditorView.lineWrapping,
        ...(languageExtension ? [languageExtension] : []),
        EditorView.updateListener.of((update) => {
          if (update.docChanged) {
            opts.onChanged(update.state.doc.toString());
            opts.onBlocksChanged(update.state.field(conflictField).blocks);
          }
        }),
      ],
    }),
    parent: host,
  });

  active = { view, labels: opts.labels };
  view.dispatch({ effects: setBlocks.of(opts.blocks) });

  const handle: ConflictEditorHandle = {
    getBlocks: () => view.state.field(conflictField).blocks,
    getText: () => view.state.doc.toString(),
    focus: () => view.focus(),
    destroy: () => {
      if (activeHandle === handle) activeHandle = null;
      if (active?.view === view) active = null;
      view.destroy();
    },
    gotoBlock(index: number): void {
      const blocks = view.state.field(conflictField).blocks;
      const b = blocks[Math.max(0, Math.min(index, blocks.length - 1))];
      if (!b) return;
      const line = view.state.doc.line(Math.min(b.start + 1, view.state.doc.lines));
      view.dispatch({
        selection: { anchor: line.from },
        effects: EditorView.scrollIntoView(line.from, { y: "center" }),
      });
      view.focus();
    },
    accept(index: number, side: BlockSide): void {
      const blocks = view.state.field(conflictField).blocks;
      const b = blocks[index];
      if (!b) return;
      const changes: { from: number; to: number }[] = [];
      const removeRange = (fromLine: number, toLine: number): void => {
        const from = view.state.doc.line(Math.min(fromLine + 1, view.state.doc.lines)).from;
        const to =
          toLine >= view.state.doc.lines
            ? view.state.doc.length
            : view.state.doc.line(toLine + 1).from;
        if (to > from) changes.push({ from, to });
      };
      if (side === "current") {
        // <<<<<<<
        removeRange(b.start, b.current_start);
        if (b.base_start !== null && b.base_end !== null) {
          removeRange(b.base_start - 1, b.base_end); // ||||||| + base
        }
        removeRange(b.incoming_start - 1, b.incoming_start); // =======
        removeRange(b.incoming_start, b.end + 1); // theirs + >>>>>>>
      } else if (side === "incoming") {
        // <<<<<<< + ours + (||||||||| + base) + =======
        removeRange(b.start, b.incoming_start);
        removeRange(b.end, b.end + 1); // >>>>>>>
      } else {
        // Both: ours + theirs back to back, drop markers and base section.
        removeRange(b.start, b.start + 1); // <<<<<<<
        if (b.base_start !== null && b.base_end !== null) {
          removeRange(b.current_end, b.base_end); // ||||||| + base
        } else {
          removeRange(b.current_end, b.incoming_start); // =======
        }
        removeRange(b.end, b.end + 1); // >>>>>>>
      }
      view.dispatch({ changes });
    },
  };
  activeHandle = handle;

  return handle;
}

/** Language mode via @codemirror/language-data lazy loading (v1 may degrade). */
async function detectLanguage(path: string) {
  try {
    const desc = LanguageDescription.matchFilename(languages, path);
    if (!desc) return null;
    return await desc.load();
  } catch {
    return null; // degrade to plain text (ADR-010)
  }
}
