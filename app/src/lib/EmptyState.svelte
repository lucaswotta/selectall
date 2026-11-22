<script lang="ts">
  import type { Snippet } from "svelte";
  import Asterisk from "./Asterisk.svelte";

  let { title, children, actions }: { title: string; children?: Snippet; actions?: Snippet } = $props();
</script>

<section class="empty-state">
  <span class="mark"><Asterisk size={40} /></span>
  <h2>{title}</h2>
  {#if children}<div class="text">{@render children()}</div>{/if}
  {#if actions}<div class="actions">{@render actions()}</div>{/if}
</section>

<style>
  .empty-state { margin: auto; padding: 40px; display: grid; justify-items: center; gap: 6px; text-align: center; max-width: 60ch; }
  .mark { color: var(--border-strong); display: flex; }
  h2 { color: var(--text); font-size: var(--text-lg); font-weight: 600; margin: 4px 0 0; }
  .text { color: var(--text-muted); font-size: var(--text-md); }
  .text :global(p) { margin: 0 0 6px; }
  .text :global(ol) { text-align: left; margin: 8px 0 0; padding-left: 20px; display: grid; gap: 4px; }
  .actions { display: flex; gap: 8px; margin-top: 12px; flex-wrap: wrap; justify-content: center; }
</style>
