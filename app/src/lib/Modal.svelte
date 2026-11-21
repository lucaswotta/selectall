<script lang="ts">
  import type { Snippet } from "svelte";

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
</script>

<svelte:window onkeydown={(event) => event.key === "Escape" && onclose()} />

<!-- svelte-ignore a11y_click_events_have_key_events, a11y_no_static_element_interactions -->
<div class="backdrop" onclick={(event) => event.target === event.currentTarget && onclose()}>
  <div class="modal" role="dialog" aria-modal="true" aria-labelledby={id} style:width="{width}px">
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
  .modal { max-width: calc(100vw - 32px); max-height: calc(100vh - 64px); display: flex; flex-direction: column; background: var(--surface); border-radius: var(--radius-lg); box-shadow: var(--shadow); }
  header { padding: 18px 20px 0; flex: none; }
  h2 { margin: 0 0 4px; font-size: var(--text-xl); font-weight: 600; }
  .sub { font-size: var(--text-sm); }
  .body { padding: 16px 20px; overflow: auto; min-height: 0; }
  footer { padding: 14px 20px; border-top: 1px solid var(--border); display: flex; justify-content: flex-end; gap: 10px; align-items: center; flex: none; }
</style>
