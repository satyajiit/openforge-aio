import { useMemo } from "react";
import CodeMirror from "@uiw/react-codemirror";
import { StreamLanguage } from "@codemirror/language";
import { lua } from "@codemirror/legacy-modes/mode/lua";
import { EditorView, Decoration, type DecorationSet } from "@codemirror/view";
import { StateField, type Extension } from "@codemirror/state";

export type LuaEditorProps = {
  value: string;
  onChange: (next: string) => void;
  /** When provided, the editor renders a red squiggly highlight on the line. */
  errorLine?: number | null;
  readOnly?: boolean;
};

/**
 * Decoration extension factory: paints `.cm-error-line` on the single 1-indexed
 * line passed in. We re-instantiate the StateField whenever the line changes
 * (cheap; it just re-runs `from`), which keeps the editor's transaction
 * pipeline blissfully unaware of React props.
 */
function errorLineExtension(line: number | null | undefined): Extension {
  if (!line || line < 1) return [];
  const deco = Decoration.line({ class: "cm-error-line" });
  return StateField.define<DecorationSet>({
    create(state) {
      const doc = state.doc;
      if (line > doc.lines) return Decoration.none;
      const lineObj = doc.line(line);
      return Decoration.set([deco.range(lineObj.from)]);
    },
    update(value, tr) {
      if (!tr.docChanged) return value;
      const doc = tr.state.doc;
      if (line > doc.lines) return Decoration.none;
      const lineObj = doc.line(line);
      return Decoration.set([deco.range(lineObj.from)]);
    },
    provide: (f) => EditorView.decorations.from(f),
  });
}

/** Monochrome highlight theme — no chromatic colors. Keywords / strings /
 *  comments are tonal variations of the foreground so the editor reads as
 *  part of the OpenForge aesthetic, not a foreign chunk of IDE chrome. */
const monoTheme: Extension = EditorView.theme(
  {
    "&": {
      backgroundColor: "transparent",
      color: "var(--color-foreground)",
      fontSize: "12.5px",
      fontFamily:
        '"JetBrains Mono", "Fira Code", ui-monospace, SFMono-Regular, Menlo, Consolas, monospace',
    },
    ".cm-content": { caretColor: "var(--color-foreground)" },
    ".cm-gutters": {
      backgroundColor: "transparent",
      color:
        "color-mix(in oklch, var(--color-foreground) 35%, transparent)",
      borderRight:
        "1px solid color-mix(in oklch, var(--color-border) 80%, transparent)",
    },
    ".cm-activeLine": {
      backgroundColor:
        "color-mix(in oklch, var(--color-foreground) 4%, transparent)",
    },
    ".cm-activeLineGutter": {
      backgroundColor: "transparent",
      color: "var(--color-foreground)",
    },
    ".cm-cursor, .cm-dropCursor": {
      borderLeftColor: "var(--color-foreground)",
    },
  },
  { dark: false },
);

export function LuaEditor({
  value,
  onChange,
  errorLine,
  readOnly,
}: LuaEditorProps) {
  const extensions = useMemo<Extension[]>(
    () => [
      StreamLanguage.define(lua),
      monoTheme,
      EditorView.lineWrapping,
      errorLineExtension(errorLine ?? null),
    ],
    [errorLine],
  );

  return (
    <CodeMirror
      value={value}
      onChange={onChange}
      extensions={extensions}
      readOnly={readOnly}
      height="100%"
      style={{ height: "100%", overflow: "auto" }}
      basicSetup={{
        lineNumbers: true,
        foldGutter: true,
        highlightActiveLine: true,
        highlightActiveLineGutter: true,
        bracketMatching: true,
        autocompletion: false,
        searchKeymap: true,
      }}
    />
  );
}
