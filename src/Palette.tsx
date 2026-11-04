import { useEffect, useMemo, useRef, useState } from "react";

export type Cmd = { id: string; label: string; group: string; shortcut?: string; run: () => void };

/** Letters of `q` appear in `s` in order. */
function matches(s: string, q: string) {
  let i = 0;
  for (const c of s.toLowerCase()) if (c === q[i]) i++;
  return i === q.length;
}

/** ⌘K: type to filter every action and post, Enter to run. */
export function Palette({ commands, onClose }: { commands: Cmd[]; onClose: () => void }) {
  const [q, setQ] = useState("");
  const [i, setI] = useState(0);
  const list = useMemo(() => commands.filter((c) => matches(c.label, q.toLowerCase().replace(/\s+/g, ""))), [commands, q]);
  const active = useRef<HTMLButtonElement>(null);
  useEffect(() => {
    active.current?.scrollIntoView({ block: "nearest" });
  }, [i]);
  const run = (c: Cmd | undefined) => {
    if (!c) return;
    onClose();
    c.run();
  };

  return (
    <div className="palette-bg" onMouseDown={onClose}>
      <div className="palette" onMouseDown={(e) => e.stopPropagation()}>
        <input
          autoFocus
          placeholder="Type a command or post title"
          value={q}
          onChange={(e) => {
            setQ(e.target.value);
            setI(0);
          }}
          onKeyDown={(e) => {
            if (e.key === "ArrowDown") setI((x) => Math.min(x + 1, list.length - 1));
            else if (e.key === "ArrowUp") setI((x) => Math.max(x - 1, 0));
            else if (e.key === "Enter") run(list[i]);
            else if (e.key === "Escape") onClose();
            else return;
            e.preventDefault();
          }}
        />
        <div className="palette-list">
          {list.map((c, n) => (
            <button
              key={c.id}
              ref={n === i ? active : undefined}
              className="palette-item"
              aria-selected={n === i}
              onMouseMove={() => setI(n)}
              onClick={() => run(c)}
            >
              <span className="muted palette-group">{c.group}</span>
              <span className="grow">{c.label}</span>
              {c.shortcut && <kbd>{c.shortcut}</kbd>}
            </button>
          ))}
          {!list.length && <div className="hint pad">No matches</div>}
        </div>
      </div>
    </div>
  );
}
