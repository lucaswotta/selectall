<script lang="ts">
  let { onnavigate }: { onnavigate: (view: "consulta" | "frota" | "salvas" | "historico" | "auditoria") => void } = $props();

  const sections = [
    { id: "inicio", title: "Primeiros passos" },
    { id: "consultar", title: "Consultar" },
    { id: "corrigir", title: "Corrigir" },
    { id: "salvas", title: "Consultas salvas" },
    { id: "historico", title: "Histórico e auditoria" },
    { id: "seguranca", title: "Senhas e segurança" },
    { id: "atalhos", title: "Atalhos de teclado" },
    { id: "problemas", title: "Quando algo não responde" },
  ];

  let content = $state<HTMLElement>();

  function go(id: string) {
    const target = content?.querySelector<HTMLElement>(`#ajuda-${id}`);
    target?.scrollIntoView({ block: "start" });
    target?.focus({ preventScroll: true });
  }
</script>

<div class="help">
  <nav class="toc" aria-label="Tópicos da ajuda">
    <h2 class="section-title">Como usar</h2>
    {#each sections as section (section.id)}
      <button class="toc-item" onclick={() => go(section.id)}>{section.title}</button>
    {/each}
  </nav>

  <article class="content selectable" bind:this={content}>
    <p class="lead">O SelectAll roda o mesmo SQL nos bancos de todos os PDVs, concentradores e servidores das lojas escolhidas e junta o resultado numa grade só, com loja e equipamento em cada linha.</p>

    <section id="ajuda-inicio" tabindex="-1">
      <h3>Primeiros passos</h3>
      <ol>
        <li><strong>Rede.</strong> A empresa cujas lojas você atende. Troque, renomeie ou crie outra pelo seletor no alto da tela.</li>
        <li><strong>Perfil de sistema.</strong> Em Frota, botão Perfis de sistema: motor (MySQL/MariaDB, PostgreSQL ou SQL Server), porta, banco, usuário e senha. A senha vai para o cofre do sistema operacional, nunca para arquivo.</li>
        <li><strong>Lojas e equipamentos.</strong> Em Frota, botão Adicionar lojas. Quatro caminhos, todos com prévia antes de gravar:
          <ul>
            <li>Regra de rede: um modelo como <code>10.{"{sub}"}.3.1-50</code> por tipo de equipamento e a lista de lojas com a sub-rede de cada uma.</li>
            <li>Colar lista: IPs, faixas (<code>10.20.3.1-12</code>) ou linhas <code>loja;ip;tipo;nome</code>.</li>
            <li>Planilha: CSV ou XLSX com colunas como loja, ip, tipo, nome e perfil.</li>
            <li>Varredura: testa a porta numa faixa e mostra quem responde, com a versão do servidor.</li>
          </ul>
        </li>
      </ol>
      <button class="btn" onclick={() => onnavigate("frota")}>Ir para a Frota</button>
    </section>

    <section id="ajuda-consultar" tabindex="-1">
      <h3>Consultar</h3>
      <ol>
        <li>Na coluna Alvos, marque as lojas. A seta ao lado de cada loja abre seus equipamentos para desmarcar um a um. Os botões de tipo (PDV, Concentrador) escondem ou mostram um tipo inteiro.</li>
        <li>Escreva o SQL no editor. Com um trecho selecionado, só o trecho é executado.</li>
        <li>Execute com o botão ou <span class="kbd">Ctrl</span> + <span class="kbd">Enter</span>. A leitura roda em transação somente leitura: o servidor recusa qualquer alteração.</li>
        <li>Acompanhe o progresso por loja. Cada equipamento traz até 1.000 linhas.</li>
      </ol>
      <p>O resultado tem quatro abas:</p>
      <ul>
        <li><strong>Resultado:</strong> a grade consolidada, com filtro, ordenação por coluna, agrupamento por loja, cópia e exportação em CSV ou XLSX para a pasta Downloads.</li>
        <li><strong>Comparar:</strong> agrupa os equipamentos por resultado idêntico e destaca quem diverge da maioria. Útil para achar o PDV com versão ou parâmetro diferente.</li>
        <li><strong>Offline e falhas:</strong> quem não respondeu, com o motivo em português, e o botão Repetir só estes.</li>
        <li><strong>Mensagens:</strong> o registro da execução, com perfil usado, avisos do servidor e cortes por limite.</li>
      </ul>
    </section>

    <section id="ajuda-corrigir" tabindex="-1">
      <h3>Corrigir</h3>
      <p>Alterações passam por três etapas, no modo Corrigir:</p>
      <ol>
        <li><strong>Ensaio:</strong> executa em transação em todos os alvos e desfaz. Mostra quantas linhas cada equipamento alteraria e destaca quem foge do padrão.</li>
        <li><strong>Piloto:</strong> aplica de verdade em um equipamento escolhido. Confira nele antes de seguir.</li>
        <li><strong>Lote:</strong> aplica no restante, uma transação por equipamento. Falhas são desfeitas e podem ser repetidas. Para confirmar, digite o número de equipamentos.</li>
      </ol>
      <p>São recusados <code>UPDATE</code> e <code>DELETE</code> sem <code>WHERE</code> que cite uma coluna, e comandos que mudam a estrutura (<code>ALTER</code>, <code>DROP</code>, <code>CREATE</code>, <code>TRUNCATE</code>), porque no MySQL eles confirmam sozinhos e não podem ser desfeitos.</p>
      <p>Pular ensaio fica desligado por padrão. Sem ensaio não há prévia de linhas; use só quando o ensaio não for possível.</p>
    </section>

    <section id="ajuda-salvas" tabindex="-1">
      <h3>Consultas salvas</h3>
      <p>Salve um SQL pelo botão Salvar consulta. Parâmetros como <code>{"{{codigo_produto}}"}</code> viram campos pedidos antes de executar. O valor entra sempre como texto citado no dialeto de cada motor, nunca cru no SQL.</p>
      <p>Uma consulta de escrita abre direto no modo Corrigir. Exporte a biblioteca para compartilhar com a equipe; ao importar, consulta com o mesmo nome é substituída.</p>
      <button class="btn" onclick={() => onnavigate("salvas")}>Ir para Salvas</button>
    </section>

    <section id="ajuda-historico" tabindex="-1">
      <h3>Histórico e auditoria</h3>
      <p><strong>Histórico:</strong> cada leitura fica guardada com SQL, valores, alvos e dados de cada equipamento. Reabra para ver a mesma grade, comparar ou repetir as falhas. O padrão guarda 30 dias ou 500 MB; mude em Ajustes. SQL que cita senha, token ou segredo não é guardado.</p>
      <p><strong>Auditoria:</strong> toda etapa de correção, com quem, quando, SQL, valores e o resultado de cada equipamento. O registro só aceita acréscimo e pode ser exportado em CSV ou XLSX.</p>
      <div class="row">
        <button class="btn" onclick={() => onnavigate("historico")}>Ir para o Histórico</button>
        <button class="btn" onclick={() => onnavigate("auditoria")}>Ir para a Auditoria</button>
      </div>
    </section>

    <section id="ajuda-seguranca" tabindex="-1">
      <h3>Senhas e segurança</h3>
      <ul>
        <li>Senhas ficam no cofre do sistema: Gerenciador de Credenciais no Windows, Secret Service no Linux. A exportação da frota e da biblioteca não leva senhas.</li>
        <li>Para consultar, use no perfil um usuário de banco só com <code>SELECT</code>. Um usuário com escrita só é preciso para Corrigir.</li>
        <li>Um equipamento pode ter mais de um perfil em ordem: se o primeiro for recusado, o próximo é tentado, e a grade mostra qual respondeu.</li>
        <li>Em Ajustes, limite quantos equipamentos da mesma loja rodam ao mesmo tempo para não saturar links lentos.</li>
      </ul>
    </section>

    <section id="ajuda-atalhos" tabindex="-1">
      <h3>Atalhos de teclado</h3>
      <table class="keys">
        <tbody>
          <tr><td><span class="kbd">Ctrl</span> + <span class="kbd">Enter</span></td><td>Executar, ou fazer o ensaio no modo Corrigir</td></tr>
          <tr><td><span class="kbd">Ctrl</span> + <span class="kbd">F</span></td><td>Buscar no editor de SQL</td></tr>
          <tr><td><span class="kbd">Ctrl</span> + <span class="kbd">Z</span></td><td>Desfazer no editor</td></tr>
          <tr><td><span class="kbd">Tab</span></td><td>Indentar no editor; nos demais campos, ir ao próximo controle</td></tr>
          <tr><td><span class="kbd">Esc</span> e depois <span class="kbd">Tab</span></td><td>Sair do editor pelo teclado</td></tr>
          <tr><td><span class="kbd">Esc</span></td><td>Fechar a janela aberta</td></tr>
          <tr><td><span class="kbd">Enter</span> ou <span class="kbd">Espaço</span></td><td>Escolher a loja na tabela da Frota</td></tr>
        </tbody>
      </table>
    </section>

    <section id="ajuda-problemas" tabindex="-1">
      <h3>Quando algo não responde</h3>
      <ul>
        <li><strong>Offline:</strong> o equipamento não atendeu na porta do perfil. É comum fora do horário da loja. Use Repetir só estes mais tarde.</li>
        <li><strong>Usuário ou senha recusados:</strong> confira o perfil em Frota. Perfil sem senha guardada aparece com aviso na Consulta.</li>
        <li><strong>Prazo esgotado:</strong> a consulta passou de 30 segundos no equipamento e foi interrompida.</li>
        <li><strong>Falha ao negociar TLS com SQL Server 2008 ou 2012:</strong> sem atualização, eles só oferecem TLS 1.0, que o SelectAll não aceita. Atualize o servidor ou habilite TLS 1.2.</li>
        <li><strong>Resultado desconhecido numa correção:</strong> a conexão caiu durante o COMMIT. Confira no equipamento antes de repetir.</li>
      </ul>
    </section>
  </article>
</div>

<style>
  .help { flex: 1; min-height: 0; display: grid; grid-template-columns: 240px 1fr; }
  .toc { border-right: 1px solid var(--border); padding: 20px 12px; display: flex; flex-direction: column; gap: 2px; overflow: auto; }
  .toc .section-title { margin: 0 8px 8px; }
  .toc-item { border: 0; background: transparent; text-align: left; padding: 6px 8px; border-radius: var(--radius); font-size: var(--text-sm); color: var(--text-muted); cursor: pointer; }
  .toc-item:hover { background: var(--surface-sunken); color: var(--text); }
  .content { overflow: auto; padding: 20px 32px 48px; line-height: 1.6; }
  .content > * { max-width: 820px; }
  .lead { font-size: var(--text-lg); margin: 0 0 8px; }
  section { padding-top: 16px; }
  section:focus { outline: none; }
  h3 { font-size: var(--text-lg); font-weight: 600; margin: 8px 0 8px; }
  p, li { color: var(--text); }
  ol, ul { padding-left: 22px; display: grid; gap: 6px; }
  li > ul { margin-top: 6px; }
  code { font-family: var(--font-mono); font-size: 0.92em; background: var(--surface-sunken); padding: 1px 4px; border-radius: 3px; }
  .row { display: flex; gap: 8px; }
  .keys { width: auto; }
  .keys td { height: auto; padding: 6px 16px 6px 0; border: 0; white-space: normal; }
  .keys tr:hover td { background: transparent; }
</style>
