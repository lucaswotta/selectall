<script lang="ts">
  import { onDestroy, onMount } from "svelte";
  import { Compartment, EditorState, Prec } from "@codemirror/state";
  import {
    Decoration,
    EditorView,
    MatchDecorator,
    ViewPlugin,
    drawSelection,
    highlightActiveLine,
    highlightActiveLineGutter,
    highlightSpecialChars,
    keymap,
    lineNumbers,
    placeholder as placeholderText,
    type DecorationSet,
    type ViewUpdate,
  } from "@codemirror/view";
  import { defaultKeymap, history, historyKeymap, indentWithTab } from "@codemirror/commands";
  import { HighlightStyle, bracketMatching, indentOnInput, syntaxHighlighting } from "@codemirror/language";
  import { highlightSelectionMatches, search, searchKeymap } from "@codemirror/search";
  import { MSSQL, MySQL, PostgreSQL, sql as sqlLanguage } from "@codemirror/lang-sql";
  import { tags } from "@lezer/highlight";
  import type { Engine } from "./api";

  let {
    value = $bindable(""),
    engine = "mysql",
    label = "SQL",
    onrun,
  }: { value?: string; engine?: Engine; label?: string; onrun?: () => void } = $props();

  let host = $state<HTMLDivElement>();
  let view: EditorView | undefined;
  const language = new Compartment();

  const dialects = { mysql: MySQL, postgres: PostgreSQL, sqlserver: MSSQL } as const;

  // Cores dos tokens do tema: seguem o claro e o escuro sem outra configuração.
  const highlight = HighlightStyle.define([
    { tag: [tags.keyword, tags.operatorKeyword], color: "var(--brand)", fontWeight: "500" },
    { tag: [tags.string, tags.special(tags.string)], color: "var(--ok)" },
    { tag: [tags.number, tags.bool, tags.null], color: "var(--warn)" },
    { tag: [tags.lineComment, tags.blockComment], color: "var(--text-muted)", fontStyle: "italic" },
    { tag: [tags.typeName, tags.standard(tags.name)], color: "var(--brand)" },
    { tag: tags.invalid, color: "var(--err)" },
  ]);

  const theme = EditorView.theme({
    "&": { height: "100%", fontSize: "13px", backgroundColor: "var(--surface)", color: "var(--text)" },
    "&.cm-focused": { outline: "none" },
    ".cm-scroller": { fontFamily: "var(--font-mono)", lineHeight: "20px" },
    ".cm-content": { padding: "10px 0", caretColor: "var(--text)" },
    ".cm-line": { padding: "0 12px" },
    ".cm-gutters": { backgroundColor: "var(--surface-sunken)", color: "var(--text-muted)", border: "none" },
    ".cm-lineNumbers .cm-gutterElement": { padding: "0 8px 0 12px", minWidth: "32px" },
    ".cm-activeLine": { backgroundColor: "transparent" },
    "&.cm-focused .cm-activeLine": { backgroundColor: "color-mix(in srgb, var(--brand-soft) 45%, transparent)" },
    ".cm-activeLineGutter": { backgroundColor: "transparent", color: "var(--text)" },
    ".cm-cursor, .cm-dropCursor": { borderLeftColor: "var(--text)", borderLeftWidth: "2px" },
    "&.cm-focused > .cm-scroller > .cm-selectionLayer .cm-selectionBackground, .cm-selectionBackground, ::selection": {
      backgroundColor: "var(--selection) !important",
    },
    ".cm-selectionMatch": { backgroundColor: "var(--brand-soft)" },
    ".cm-matchingBracket": { backgroundColor: "var(--brand-soft)", outline: "1px solid var(--border-strong)" },
    ".cm-placeholder": { color: "var(--text-muted)" },
    ".cm-param": { backgroundColor: "var(--brand-soft)", color: "var(--brand)", borderRadius: "3px" },
    ".cm-panels": { backgroundColor: "var(--surface-sunken)", color: "var(--text)", borderColor: "var(--border)" },
    ".cm-panels input, .cm-panels button": { fontFamily: "var(--font-ui)", fontSize: "12px" },
    ".cm-searchMatch": { backgroundColor: "var(--warn-soft)", outline: "1px solid var(--warn)" },
  });

  // `{{nome}}` em destaque: deixa claro o que será pedido antes de executar.
  const params = new MatchDecorator({ regexp: /\{\{\s*[A-Za-z_][A-Za-z0-9_]*\s*\}\}/g, decoration: Decoration.mark({ class: "cm-param" }) });
  const paramHighlight = ViewPlugin.fromClass(
    class {
      decorations: DecorationSet;
      constructor(editor: EditorView) {
        this.decorations = params.createDeco(editor);
      }
      update(update: ViewUpdate) {
        this.decorations = params.updateDeco(update, this.decorations);
      }
    },
    { decorations: (plugin) => plugin.decorations },
  );

  /** Texto selecionado; vazio quando não há seleção. */
  export function selection(): string {
    if (!view) return "";
    const { from, to } = view.state.selection.main;
    return view.state.sliceDoc(from, to);
  }

  export function focus() {
    view?.focus();
  }

  onMount(() => {
    view = new EditorView({
      parent: host,
      state: EditorState.create({
        doc: value,
        extensions: [
          lineNumbers(),
          highlightActiveLineGutter(),
          highlightSpecialChars(),
          history(),
          drawSelection(),
          indentOnInput(),
          bracketMatching(),
          highlightActiveLine(),
          highlightSelectionMatches(),
          search({ top: true }),
          syntaxHighlighting(highlight),
          language.of(sqlLanguage({ dialect: dialects[engine] })),
          paramHighlight,
          placeholderText("Escreva o SQL. Ctrl+Enter executa; com texto selecionado, executa só a seleção."),
          Prec.highest(
            // Tab indenta; Esc seguido de Tab sai do editor (padrão do CodeMirror).
            keymap.of([{ key: "Mod-Enter", run: () => (onrun?.(), true) }]),
          ),
          keymap.of([...defaultKeymap, ...historyKeymap, ...searchKeymap, indentWithTab]),
          EditorView.contentAttributes.of({ "aria-label": label, "aria-multiline": "true", spellcheck: "false" }),
          EditorView.updateListener.of((update) => {
            if (update.docChanged) value = update.state.doc.toString();
          }),
          theme,
        ],
      }),
    });
  });

  // Valor trocado de fora (consulta salva, histórico): substitui o documento inteiro.
  $effect(() => {
    const next = value;
    if (view && next !== view.state.doc.toString()) {
      view.dispatch({ changes: { from: 0, to: view.state.doc.length, insert: next } });
    }
  });

  $effect(() => {
    const dialect = dialects[engine];
    view?.dispatch({ effects: language.reconfigure(sqlLanguage({ dialect })) });
  });

  onDestroy(() => view?.destroy());
</script>

<div class="sql-editor" bind:this={host}></div>

<style>
  .sql-editor { height: 100%; min-height: 0; overflow: hidden; }
</style>
