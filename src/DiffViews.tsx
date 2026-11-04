import { useEffect, useRef, useState } from "react";
import { MergeView } from "@codemirror/merge";
import { EditorState, type Extension } from "@codemirror/state";
import { EditorView } from "@codemirror/view";
import { convertFileSrc } from "@tauri-apps/api/core";
import { LoaderCircle } from "lucide-react";
import type { Conflict } from "./api";

const readOnly = [EditorState.readOnly.of(true), EditorView.editable.of(false)];

/** Side-by-side diff in a host div; the view is rebuilt when its inputs change. */
function useMerge(
  host: React.RefObject<HTMLDivElement | null>,
  a: string | null,
  b: string | null,
  extensions: Extension[],
  editableA: boolean,
) {
  const merge = useRef<MergeView | null>(null);
  useEffect(() => {
    if (a === null || b === null || !host.current) return;
    const mv = new MergeView({
      a: { doc: a, extensions: editableA ? extensions : [...extensions, ...readOnly] },
      b: { doc: b, extensions: [...extensions, ...readOnly] },
      parent: host.current,
      // Arrows between the sides copy a remote chunk into your version.
      revertControls: editableA ? "b-to-a" : undefined,
      highlightChanges: true,
      gutter: true,
      collapseUnchanged: editableA ? undefined : { margin: 3, minSize: 6 },
    });
    merge.current = mv;
    return () => {
      mv.destroy();
      merge.current = null;
    };
  }, [host, a, b, extensions, editableA]);
  return merge;
}

/** Shown in place of the editor while a pull waits for its conflicts to be resolved. */
export function ConflictView({
  conflict,
  localDir,
  extensions,
  busy,
  onResolve,
  onAbort,
}: {
  conflict: Conflict;
  localDir: string;
  extensions: Extension[];
  busy: boolean;
  onResolve: (text: string | null, remoteImages: string[]) => void;
  onAbort: () => void;
}) {
  const host = useRef<HTMLDivElement>(null);
  const tc = conflict.text;
  const merge = useMerge(host, tc?.ours ?? null, tc?.theirs ?? null, extensions, true);
  const [remoteImages, setRemoteImages] = useState<string[]>([]);
  const setA = (text: string) => {
    const a = merge.current?.a;
    a?.dispatch({ changes: { from: 0, to: a.state.doc.length, insert: text } });
  };

  return (
    <section className="conflict">
      <div className="conflict-head">
        <div className="grow">
          <strong>Pull found conflicts with {conflict.upstream}.</strong>{" "}
          <span className="muted">
            Your draft hasn't changed. Remote changes that didn't clash are already merged into the left side.
          </span>
        </div>
        <button className="btn small" onClick={onAbort} disabled={busy}>
          Abort
        </button>
        <button
          className="btn small primary"
          disabled={busy}
          onClick={() => onResolve(merge.current ? merge.current.a.state.doc.toString() : null, remoteImages)}
        >
          {busy && <LoaderCircle size={12} className="spin" />} Resolve
        </button>
      </div>
      {tc && (
        <>
          <div className="conflict-cols">
            <span>
              Yours <span className="muted">· editable, this becomes the draft</span>
              <button className="link-btn" onClick={() => setA(tc.ours)}>
                Keep all mine
              </button>
            </span>
            <span>
              Remote <span className="muted">· {conflict.upstream}</span>
              <button className="link-btn" onClick={() => setA(tc.theirs)}>
                Take all remote
              </button>
            </span>
          </div>
          <div className="merge" ref={host} />
        </>
      )}
      {conflict.binary.length > 0 && (
        <div className="conflict-images">
          <span className="label">Images changed on both sides</span>
          {conflict.binary.map((name) => {
            const remote = remoteImages.includes(name);
            const pick = (r: boolean) => setRemoteImages((xs) => (r ? [...xs, name] : xs.filter((x) => x !== name)));
            return (
              <div key={name} className="img-choice">
                <span className="mono">{name}</span>
                <button className={`img-pick ${!remote ? "on" : ""}`} onClick={() => pick(false)}>
                  <img src={convertFileSrc(`${localDir}/${name}`)} alt="" /> Yours
                </button>
                <button className={`img-pick ${remote ? "on" : ""}`} onClick={() => pick(true)}>
                  <img src={convertFileSrc(`${conflict.remoteDir}/${name}`)} alt="" /> Remote
                </button>
              </div>
            );
          })}
        </div>
      )}
    </section>
  );
}

/** Read-only diff of the remote (as last synced) against the draft: what Push would send. */
export function ReviewView({
  base,
  current,
  upstream,
  extensions,
  onClose,
}: {
  base: string;
  current: string;
  upstream: string;
  extensions: Extension[];
  onClose: () => void;
}) {
  const host = useRef<HTMLDivElement>(null);
  useMerge(host, base, current, extensions, false);
  return (
    <section className="conflict">
      <div className="conflict-head">
        <div className="grow">
          <strong>Review changes</strong> <span className="muted">· what Push sends to GitHub</span>
        </div>
        <button className="btn small" onClick={onClose}>
          Close
        </button>
      </div>
      <div className="conflict-cols">
        <span>
          {upstream} <span className="muted">· last synced</span>
        </span>
        <span>Your draft</span>
      </div>
      <div className="merge" ref={host} />
    </section>
  );
}
