import { syntaxTree } from "@codemirror/language";
import type { Range } from "@codemirror/state";
import { Decoration, type DecorationSet, EditorView, ViewPlugin, type ViewUpdate, WidgetType } from "@codemirror/view";

/** Turns an image URL from the markdown into one the webview can load, or null to leave the source showing. */
export type ImageResolver = (url: string) => string | null;

class ImageWidget extends WidgetType {
  constructor(
    readonly src: string,
    readonly alt: string,
  ) {
    super();
  }
  eq(other: ImageWidget) {
    return other.src === this.src && other.alt === this.alt;
  }
  toDOM() {
    const img = document.createElement("img");
    img.className = "cm-lp-image";
    img.src = this.src;
    img.alt = this.alt;
    img.title = this.alt;
    return img;
  }
}

const hide = Decoration.replace({});

/**
 * Obsidian-style live preview: on every line without a cursor or selection, hide markdown
 * syntax (heading marks, emphasis, inline code ticks, link URLs) and show images inline.
 * Only decorations change; the document is still the plain markdown that gets published.
 */
export function livePreview(resolveImage: ImageResolver) {
  function build(view: EditorView): DecorationSet {
    const { state } = view;
    const active = new Set<number>();
    for (const r of state.selection.ranges) {
      for (let n = state.doc.lineAt(r.from).number; n <= state.doc.lineAt(r.to).number; n++) active.add(n);
    }
    const onActive = (pos: number) => active.has(state.doc.lineAt(pos).number);
    const decos: Range<Decoration>[] = [];

    for (const { from, to } of view.visibleRanges) {
      syntaxTree(state).iterate({
        from,
        to,
        enter: (node) => {
          const name = node.name;
          const heading = /^ATXHeading(\d)$/.exec(name);
          if (heading) {
            // Headings are styled on every line, the cursor line included, so the text doesn't jump.
            decos.push(Decoration.line({ class: `cm-lp-h${heading[1]}` }).range(state.doc.lineAt(node.from).from));
            return;
          }
          if (onActive(node.from)) return;

          if (name === "HeaderMark" && node.node.parent?.name.startsWith("ATXHeading")) {
            const end = state.doc.sliceString(node.to, node.to + 1) === " " ? node.to + 1 : node.to;
            decos.push(hide.range(node.from, end));
          } else if (name === "EmphasisMark" || name === "StrikethroughMark") {
            decos.push(hide.range(node.from, node.to));
          } else if (name === "CodeMark" && node.node.parent?.name === "InlineCode") {
            decos.push(hide.range(node.from, node.to));
          } else if (name === "Link") {
            const marks = node.node.getChildren("LinkMark");
            // [text](url) or [text][ref]: keep only the text between the first two marks.
            if (marks.length >= 2 && state.doc.sliceString(marks[0].from, marks[0].to) === "[") {
              decos.push(hide.range(node.from, marks[0].to));
              decos.push(Decoration.mark({ class: "cm-lp-link" }).range(marks[0].to, marks[1].from));
              if (marks[1].from < node.to) decos.push(hide.range(marks[1].from, node.to));
            }
            return false;
          } else if (name === "Image") {
            const url = node.node.getChild("URL");
            const src = url && resolveImage(state.doc.sliceString(url.from, url.to));
            if (src && state.doc.lineAt(node.from).number === state.doc.lineAt(node.to).number) {
              const alt = /^!\[([^\]]*)\]/.exec(state.doc.sliceString(node.from, node.to))?.[1] ?? "";
              decos.push(Decoration.replace({ widget: new ImageWidget(src, alt) }).range(node.from, node.to));
            }
            return false;
          }
        },
      });
    }
    return Decoration.set(decos, true);
  }

  return ViewPlugin.fromClass(
    class {
      decorations: DecorationSet;
      constructor(view: EditorView) {
        this.decorations = build(view);
      }
      update(u: ViewUpdate) {
        if (u.docChanged || u.selectionSet || u.viewportChanged || syntaxTree(u.startState) !== syntaxTree(u.state)) {
          this.decorations = build(u.view);
        }
      }
    },
    { decorations: (v) => v.decorations },
  );
}
