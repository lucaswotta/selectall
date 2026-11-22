<script lang="ts">
  import { onMount } from "svelte";
  import { api, message, type AppInfo, type Chain, type SavedView, type Theme } from "./lib/api";
  import { applyTheme, nextTheme, themeLabel } from "./lib/theme";
  import Asterisk from "./lib/Asterisk.svelte";
  import Welcome from "./views/Welcome.svelte";
  import Fleet from "./views/Fleet.svelte";
  import Query from "./views/Query.svelte";
  import Audit from "./views/Audit.svelte";
  import Saved from "./views/Saved.svelte";
  import History from "./views/History.svelte";
  import Modal from "./lib/Modal.svelte";

  type View = "consulta" | "frota" | "salvas" | "historico" | "auditoria";

  const views: { id: View; label: string; title: string; icon: string }[] = [
    { id: "consulta", label: "Consulta", title: "Consulta", icon: "M4 6h16M4 12h10M4 18h7M16 15l4 3-4 3z" },
    { id: "frota", label: "Frota", title: "Frota", icon: "M3 9l1.5-5h15L21 9M3 9h18v2a3 3 0 0 1-6 0 3 3 0 0 1-6 0 3 3 0 0 1-6 0zM5 13v7h14v-7M10 20v-4h4v4" },
    { id: "salvas", label: "Salvas", title: "Consultas salvas", icon: "M6 3h12v18l-6-4-6 4z" },
    { id: "historico", label: "Histórico", title: "Histórico", icon: "M12 3.5a8.5 8.5 0 1 1 0 17 8.5 8.5 0 0 1 0-17zM12 7.5V12l3 2" },
    { id: "auditoria", label: "Auditoria", title: "Auditoria", icon: "M12 3l8 3v6c0 4.5-3.4 8-8 9-4.6-1-8-4.5-8-9V6zM9 12l2 2 4-4" },
  ];

  let view = $state<View>("consulta");
  let chains = $state<Chain[]>([]);
  let chainId = $state<number | null>(null);
  let theme = $state<Theme>("system");
  let info = $state<AppInfo | null>(null);
  let menuOpen = $state(false);
  let loadError = $state("");
  let loaded = $state(false);
  let chainDialog = $state<"rename" | "delete" | null>(null);
  let chainName = $state("");
  let chainFailure = $state("");
  let queryView = $state<Query>();

  const chain = $derived(chains.find((c) => c.id === chainId) ?? null);
  const current = $derived(views.find((v) => v.id === view)!);

  onMount(async () => {
    try {
      [chains, theme, info] = await Promise.all([api.chains(), api.theme(), api.info()]);
      applyTheme(theme);
      chainId = chains[0]?.id ?? null;
    } catch (error) {
      loadError = message(error);
    }
    loaded = true;
  });

  async function cycleTheme() {
    const next = nextTheme[theme];
    applyTheme(next);
    theme = next;
    try {
      await api.setTheme(next);
    } catch (error) {
      loadError = message(error);
    }
  }

  function created(created: Chain) {
    chains = [...chains, created].sort((a, b) => a.name.localeCompare(b.name, "pt-BR"));
    chainId = created.id;
    menuOpen = false;
  }

  function openChainDialog(kind: "rename" | "delete") {
    chainDialog = kind;
    chainName = kind === "rename" ? (chain?.name ?? "") : "";
    chainFailure = "";
    menuOpen = false;
  }

  async function submitChain(event: SubmitEvent) {
    event.preventDefault();
    if (!chain) return;
    try {
      if (chainDialog === "rename") {
        const renamed = await api.renameChain(chain.id, chainName);
        chains = chains.map((c) => (c.id === renamed.id ? renamed : c)).sort((a, b) => a.name.localeCompare(b.name, "pt-BR"));
      } else {
        const removed = chain.id;
        try {
          await api.deleteChain(removed);
        } catch (error) {
          // A rede pode ter saído mesmo assim, se só o cofre falhou; o aviso fica na barra.
          loadError = message(error);
        }
        chains = await api.chains();
        if (chains.some((c) => c.id === removed)) {
          chainFailure = loadError;
          return;
        }
        chainId = chains[0]?.id ?? null;
      }
      chainDialog = null;
    } catch (error) {
      chainFailure = message(error);
    }
  }

  function openSaved(query: SavedView) {
    view = "consulta";
    queryView?.openSaved(query);
  }

  function reopen(id: number) {
    view = "consulta";
    queryView?.reopen(id);
  }

  function onWindowClick(event: MouseEvent) {
    if (menuOpen && !(event.target as Element).closest(".chain-picker")) menuOpen = false;
  }
</script>

<svelte:window onclick={onWindowClick} onkeydown={(e) => e.key === "Escape" && (menuOpen = false)} />

{#if !loaded}
  <div class="splash"><Asterisk size={40} /></div>
{:else if !chain}
  <Welcome
    error={loadError}
    first={chains.length === 0}
    oncreated={created}
    oncancel={() => (chainId = chains[0]?.id ?? null)}
  />
{:else}
  <div class="app">
    <nav class="rail" aria-label="Seções">
      <span class="logo"><Asterisk size={28} label="SelectAll" /></span>
      {#each views as item (item.id)}
        <button class="nav" aria-current={view === item.id ? "page" : undefined} onclick={() => (view = item.id)}>
          <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.7" stroke-linecap="round" stroke-linejoin="round"><path d={item.icon} /></svg>
          {item.label}
        </button>
      {/each}
      <span class="spacer"></span>
      <button class="nav" onclick={cycleTheme} title="Alternar tema: sistema, claro, escuro">
        <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.7"><circle cx="12" cy="12" r="8" /><path d="M12 4a8 8 0 0 1 0 16z" fill="currentColor" /></svg>
        {themeLabel[theme]}
      </button>
    </nav>

    <div class="main">
      <header class="topbar">
        <div class="chain-picker">
          <button class="chain" aria-haspopup="true" aria-expanded={menuOpen} onclick={() => (menuOpen = !menuOpen)}>
            <span class="dot"></span>{chain.name}
            <svg width="12" height="12" viewBox="0 0 12 12" fill="none" stroke="currentColor" stroke-width="1.5"><path d="m3 4.5 3 3 3-3" /></svg>
          </button>
          {#if menuOpen}
            <div class="menu" role="menu">
              {#each chains as item (item.id)}
                <button role="menuitemradio" aria-checked={item.id === chainId} onclick={() => ((chainId = item.id), (menuOpen = false))}>{item.name}</button>
              {/each}
              <hr />
              <button role="menuitem" onclick={() => ((chainId = null), (menuOpen = false))}>Adicionar rede</button>
              <button role="menuitem" onclick={() => openChainDialog("rename")}>Renomear {chain.name}</button>
              <button role="menuitem" class="danger" onclick={() => openChainDialog("delete")}>Excluir {chain.name}</button>
            </div>
          {/if}
        </div>
        <h1>{current.title}</h1>
        {#if loadError}<span class="pill err">{loadError}</span>{/if}
        <span class="version muted">v{info?.version}</span>
      </header>
      <!-- A Consulta fica montada para manter SQL e resultado ao trocar de seção. -->
      {#key chain.id}<Query bind:this={queryView} chainId={chain.id} active={view === "consulta"} onaudit={() => (view = "auditoria")} />{/key}
      {#if view === "frota"}
        {#key chain.id}<Fleet chainId={chain.id} />{/key}
      {:else if view === "salvas"}
        <Saved chainId={chain.id} onopen={openSaved} />
      {:else if view === "historico"}
        <History chainId={chain.id} onreopen={reopen} />
      {:else if view === "auditoria"}
        <Audit />
      {/if}
    </div>
  </div>
{/if}

{#if chainDialog && chain}
  <Modal title={chainDialog === "rename" ? "Renomear rede" : `Excluir ${chain.name}?`} width={480} onclose={() => (chainDialog = null)}>
    <form id="chain-form" onsubmit={submitChain}>
      {#if chainDialog === "rename"}
        <label class="field">
          <span class="label">Nome da rede</span>
          <!-- svelte-ignore a11y_autofocus -->
          <input class="input" bind:value={chainName} maxlength="80" autofocus />
        </label>
      {:else}
        <p class="confirm-text">Lojas, equipamentos, perfis e a regra da rede serão apagados, e as senhas dos perfis saem do cofre. Não dá para desfazer.</p>
        <label class="field">
          <span class="label">Digite o nome da rede para confirmar</span>
          <input class="input" bind:value={chainName} />
        </label>
      {/if}
      {#if chainFailure}<p class="error-text" role="alert">{chainFailure}</p>{/if}
    </form>
    {#snippet footer()}
      <button class="btn" onclick={() => (chainDialog = null)}>Cancelar</button>
      {#if chainDialog === "rename"}
        <button class="btn primary" form="chain-form" disabled={!chainName.trim()}>Salvar</button>
      {:else}
        <button class="btn primary danger-fill" form="chain-form" disabled={chainName.trim() !== chain?.name}>Excluir rede</button>
      {/if}
    {/snippet}
  </Modal>
{/if}

<style>
  .splash { height: 100%; display: grid; place-items: center; color: var(--brand); }
  .app { display: grid; grid-template-columns: 76px 1fr; height: 100%; }
  .rail {
    background: var(--surface);
    border-right: 1px solid var(--border);
    display: flex;
    flex-direction: column;
    align-items: center;
    padding: 14px 0;
    gap: 4px;
  }
  .logo { color: var(--brand); margin-bottom: 14px; display: flex; }
  .nav {
    width: 64px;
    border: 0;
    background: transparent;
    border-radius: var(--radius);
    padding: 8px 0 6px;
    display: flex;
    flex-direction: column;
    align-items: center;
    gap: 4px;
    font-size: 11px;
    color: var(--text-muted);
    cursor: pointer;
  }
  .nav svg { width: 20px; height: 20px; }
  .nav:hover { background: var(--surface-sunken); color: var(--text); }
  .nav[aria-current="page"] { background: var(--brand-soft); color: var(--brand); font-weight: 500; }
  .spacer { flex: 1; }
  .main { display: flex; flex-direction: column; min-width: 0; min-height: 0; }
  .topbar {
    height: 52px;
    flex: none;
    display: flex;
    align-items: center;
    gap: 16px;
    padding: 0 20px;
    border-bottom: 1px solid var(--border);
    background: var(--surface);
  }
  .topbar h1 { font-size: var(--text-lg); font-weight: 600; margin: 0; }
  .version { margin-left: auto; font-size: var(--text-xs); }
  .chain-picker { position: relative; }
  .chain {
    display: flex;
    align-items: center;
    gap: 8px;
    border: 1px solid var(--border);
    background: var(--surface);
    border-radius: var(--radius);
    padding: 5px 10px;
    cursor: pointer;
    font-weight: 500;
  }
  .dot { width: 8px; height: 8px; border-radius: 2px; background: var(--brand); }
  .menu {
    position: absolute;
    top: calc(100% + 6px);
    left: 0;
    background: var(--surface);
    border: 1px solid var(--border);
    border-radius: var(--radius-lg);
    box-shadow: var(--shadow);
    padding: 6px;
    width: 260px;
    z-index: 20;
  }
  .menu button {
    display: block;
    width: 100%;
    border: 0;
    background: transparent;
    padding: 8px 10px;
    border-radius: var(--radius);
    cursor: pointer;
    text-align: left;
  }
  .menu button:hover { background: var(--surface-sunken); }
  .menu button[aria-checked="true"] { color: var(--brand); font-weight: 500; }
  .menu button.danger { color: var(--err); }
  .confirm-text { margin: 0 0 12px; font-size: var(--text-sm); }
  .danger-fill { background: var(--err); border-color: var(--err); }
  .danger-fill:hover { background: var(--err); filter: brightness(0.92); }
  .menu hr { border: 0; border-top: 1px solid var(--border); margin: 6px 0; }
</style>
