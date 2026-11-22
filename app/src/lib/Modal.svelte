<script lang="ts">
  import { onMount, type Snippet } from "svelte";

  let {
    title,
    subtitle = "",
    width = 720,
    onclose,
    children,
    tabs,
    footer,
  }: {
    title: string;
    subtitle?: string;
    width?: number;
    onclose: () => void;
    children: Snippet;
    tabs?: Snippet;
    footer?: Snippet;
  } = $props();

  const id = `modal-${Math.random().toString(36).slice(2)}`;
  let box = $state<HTMLDivElement>();

  const FOCUSABLE = 'a[href], button:not([disabled]), input:not([disabled]), select:not([disabled]), textarea:not([disabled]), [tabindex]:not([tabindex="-1"]), [contenteditable="true"]';

  function focusable(): HTMLElement[] {
    return box ? [...box.querySelectorAll<HTMLElement>(FOCUSABLE)].filter((el) => el.offsetParent !== null) : [];
  }

  // Só o diálogo de cima responde a Esc e prende o Tab.
  const stack: HTMLDivElement[] = ((globalThis as { __modals?: HTMLDivElement[] }).__modals ??= []);

  onMount(() => {
    const opener = document.activeElement as HTMLElement | null;
    if (box) stack.push(box);
    // Campo com autofocus já tem o foco; senão vai para o primeiro controle do corpo, depois do rodapé.
    if (!box?.contains(document.activeElement)) {
      const inBody = focusable().find((el) => el.closest(".body"));
      (inBody ?? focusable()[0] ?? box)?.focus();
    }
    return () => {
      const index = box ? stack.lastIndexOf(box) : -1;
      if (index >= 0) stack.splice(index, 1);
      if (opener?.isConnected) opener.focus();
    };
  });

  function onkeydown(event: KeyboardEvent) {
    if (stack.at(-1) !== box) return;
    if (event.key === "Escape") {
      event.preventDefault();
      onclose();
      return;
    }
    if (event.key !== "Tab") return;
    const items = focusable();
    if (!items.length) return;
    const first = items[0];
    const last = items[items.length - 1];
    if (event.shiftKey && (document.activeElement === first || !box?.contains(document.activeElement))) {
      event.preventDefault();
      last.focus();
    } else if (!event.shiftKey && (document.activeElement === last || !box?.contains(document.activeElement))) {
      event.preventDefault();
      first.focus();
    }
  }
</script>

<svelte:window {onkeydown} />

<!-- svelte-ignore a11y_click_events_have_key_events, a11y_no_static_element_interactions -->
<div class="backdrop" onclick={(event) => event.target === event.currentTarget && onclose()}>
  <div class="modal" role="dialog" aria-modal="true" aria-labelledby={id} style:width="{width}px" tabindex="-1" bind:this={box}>
    <header>
      <h2 {id}>{title}</h2>
      {#if subtitle}<div class="muted sub">{subtitle}</div>{/if}
    </header>
    {#if tabs}{@render tabs()}{/if}
    <div class="body">{@render children()}</div>
    {#if footer}<footer>{@render footer()}</footer>{/if}
  </div>
</div>

<style>
  .backdrop { position: fixed; inset: 0; background: rgb(10 20 22 / 45%); display: grid; place-items: center; z-index: 40; }
  .modal:focus { outline: none; }
  .modal { max-width: calc(100vw - 32px); max-height: calc(100vh - 64px); display: flex; flex-direction: column; background: var(--surface); border-radius: var(--radius-lg); box-shadow: var(--shadow); }
  header { padding: 18px 20px 0; flex: none; }
  h2 { margin: 0 0 4px; font-size: var(--text-xl); font-weight: 600; }
  .sub { font-size: var(--text-sm); }
  .body { padding: 16px 20px; overflow: auto; min-height: 0; }
  footer { padding: 14px 20px; border-top: 1px solid var(--border); display: flex; justify-content: flex-end; gap: 10px; align-items: center; flex: none; }
</style>
