import { Chunk } from "@codemirror/merge";
import { RangeSet, StateEffect, StateField, Text, type EditorState, type Extension, type Range } from "@codemirror/state";
import { Decoration, EditorView, GutterMarker, WidgetType, gutter, type DecorationSet } from "@codemirror/view";

/**
 * Code-editor style change markers: a bar in the gutter for lines added (green) or changed (blue) since
 * `original`, and a small marker (red) where lines were removed. Clicking a marker shows the old text for
 * that change with a Revert button.
 */
export function changeMarkers(original: string, label: string): Extension {
  const orig = Text.of(original.split("\n"));
  const toggle = StateEffect.define<number>();

  type Value = { chunks: readonly Chunk[]; open: number | null };
  const field = StateField.define<Value>({
    create: (state) => ({ chunks: Chunk.build(orig, state.doc), open: null }),
    update(v, tr) {
      let { chunks, open } = v;
      if (tr.docChanged) {
        chunks = Chunk.build(orig, tr.state.doc);
        open = open === null ? null : tr.changes.mapPos(open);
        if (!chunks.some((c) => c.fromB === open)) open = null;
      }
      for (const e of tr.effects) if (e.is(toggle)) open = open === e.value ? null : e.value;
      return chunks === v.chunks && open === v.open ? v : { chunks, open };
    },
    provide: (f) => EditorView.decorations.from(f, (v) => oldText(v)),
  });

  /** Reverts a chunk to the original text; the same arithmetic as @codemirror/merge's rejectChunk. */
  function revert(view: EditorView, ch: Chunk) {
    const { state } = view;
    let insert = orig.sliceString(ch.fromA, Math.max(ch.fromA, ch.toA - 1));
    if (ch.fromA !== ch.toA && ch.toB <= state.doc.length) insert += state.lineBreak;
    view.dispatch({ changes: { from: ch.fromB, to: Math.min(state.doc.length, ch.toB), insert }, userEvent: "revert" });
  }

  class OldText extends WidgetType {
    constructor(readonly ch: Chunk) {
      super();
    }
    eq(o: OldText) {
      return o.ch.fromA === this.ch.fromA && o.ch.toA === this.ch.toA && o.ch.fromB === this.ch.fromB;
    }
    toDOM(view: EditorView) {
      const box = document.createElement("div");
      box.className = "cm-old";
      const head = document.createElement("div");
      head.className = "cm-old-head";
      const added = this.ch.fromA === this.ch.toA;
      head.textContent = added ? `Added since ${label}` : `Before (${label})`;
      const btn = document.createElement("button");
      btn.textContent = "Revert";
      btn.onmousedown = (e) => {
        e.preventDefault();
        revert(view, this.ch);
      };
      head.append(btn);
      box.append(head);
      if (!added) {
        const pre = document.createElement("pre");
        pre.textContent = orig.sliceString(this.ch.fromA, this.ch.endA);
        box.append(pre);
      }
      return box;
    }
    ignoreEvent() {
      return true;
    }
  }

  function oldText(v: Value): DecorationSet {
    const ch = v.chunks.find((c) => c.fromB === v.open);
    if (!ch) return Decoration.none;
    return Decoration.set([Decoration.widget({ widget: new OldText(ch), block: true, side: -1 }).range(ch.fromB)]);
  }

  class Marker extends GutterMarker {
    constructor(readonly kind: "added" | "modified" | "deleted") {
      super();
    }
    eq(o: Marker) {
      return o.kind === this.kind;
    }
    toDOM() {
      const el = document.createElement("div");
      el.className = `cm-change-${this.kind}`;
      return el;
    }
  }
  const marks = { added: new Marker("added"), modified: new Marker("modified"), deleted: new Marker("deleted") };

  function markers(state: EditorState) {
    const out: Range<GutterMarker>[] = [];
    for (const ch of state.field(field).chunks) {
      if (ch.fromB === ch.toB) {
        out.push(marks.deleted.range(state.doc.lineAt(Math.min(ch.fromB, state.doc.length)).from));
        continue;
      }
      const kind = ch.fromA === ch.toA ? marks.added : marks.modified;
      for (let n = state.doc.lineAt(ch.fromB).number; n <= state.doc.lineAt(ch.endB).number; n++) {
        out.push(kind.range(state.doc.line(n).from));
      }
    }
    return RangeSet.of(out, true);
  }

  return [
    field,
    gutter({
      class: "cm-change-gutter",
      markers: (view) => markers(view.state),
      domEventHandlers: {
        mousedown(view, line) {
          const ch = view.state.field(field).chunks.find((c) => {
            const from = view.state.doc.lineAt(Math.min(c.fromB, view.state.doc.length)).from;
            return line.from >= from && line.from <= Math.max(from, c.endB);
          });
          if (!ch) return false;
          view.dispatch({ effects: toggle.of(ch.fromB) });
          return true;
        },
      },
    }),
  ];
}
