<script lang="ts">
  import Asterisk from "../lib/Asterisk.svelte";

  let { view }: { view: string } = $props();

  const text: Record<string, [string, string]> = {
    consulta: ["Consulta", "Escolha lojas, escreva o SQL e veja todos os equipamentos numa grade só."],
    frota: ["Frota", "Lojas, equipamentos e perfis de sistema desta rede."],
    salvas: ["Consultas salvas", "SQL reutilizável por perfil, com parâmetros."],
    historico: ["Histórico", "Execuções anteriores com os dados retornados."],
    auditoria: ["Auditoria", "Toda correção aplicada, em registro somente de acréscimo."],
  };
</script>

<section class="empty">
  <Asterisk size={40} />
  <h2>{text[view][0]}</h2>
  <p class="muted">{text[view][1]}</p>
  <span class="pill run">Em construção</span>
</section>

<style>
  .empty { flex: 1; display: grid; place-content: center; justify-items: center; gap: 6px; text-align: center; color: var(--border-strong); padding: 40px; }
  h2 { color: var(--text); font-size: var(--text-lg); font-weight: 600; margin: 4px 0 0; }
  p { margin: 0 0 8px; max-width: 46ch; }
</style>
