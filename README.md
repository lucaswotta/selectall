# SelectAll

**Um SQL, todas as lojas.** Aplicativo desktop gratuito e de código aberto para a TI de varejo consultar e corrigir, de uma vez, os bancos de todos os PDVs, concentradores e servidores das lojas.

Português · [English](README.en.md) · [Español](README.es.md)

![SelectAll: um SELECT em 10 PDVs de 3 lojas, resultado numa grade só](docs/img/consulta.png)

## Por que existe

Uma rede de supermercados tem dezenas de lojas e centenas de caixas, cada um com o próprio banco. Para saber quais PDVs estão com a versão errada, ou para corrigir um cadastro em todos eles, sobram três caminhos ruins: entrar máquina por máquina, abrir chamado no fornecedor ou manter scripts caseiros.

O SelectAll começou como um desses scripts. O `querymyall`, em Python, nasceu na TI de uma rede de varejo para fazer exatamente isso: montar os IPs de cada loja a partir da sub-rede, tentar o usuário de cada sistema de PDV e juntar tudo num CSV. Funcionava, mas só para quem sabia rodar. Veio depois o QueryMass, um cliente SQL em várias conexões, que perdeu o que dava valor ao script: pensar em loja e caixa, não em conexão.

O SelectAll junta as duas coisas. A frota (rede, loja, equipamento) é o centro, ler é rápido, escrever é deliberado, e PDV desligado é tratado como o que é: normal.

## O que faz

| | |
| --- | --- |
| **Frota** | Redes, lojas, PDVs, self-checkouts, concentradores e servidores. Cadastro por regra de endereço (`10.{sub}.3.1-50`), lista colada, planilha ou varredura de rede. |
| **Consulta** | Um `SELECT` em todos os equipamentos escolhidos, em transação somente leitura. Resultado numa grade com loja, equipamento, IP e perfil em cada linha. Exporta CSV e XLSX. |
| **Comparação** | Agrupa os equipamentos por resultado e aponta quem diverge da maioria. "Quais caixas estão com o parâmetro diferente?" em um clique. |
| **Correção** | Ensaio (executa e desfaz em todos), piloto (aplica em um) e lote (aplica no restante). Bloqueia `UPDATE` sem `WHERE` e comandos que mudam estrutura. Tudo vai para a auditoria. |
| **Biblioteca** | Consultas salvas com parâmetros (`{{codigo_produto}}`), compartilháveis com a equipe. |
| **Histórico** | Cada leitura guardada com os dados de cada equipamento, para reabrir, comparar e repetir as falhas. |

Funciona com **MySQL/MariaDB, PostgreSQL e SQL Server**, com drivers nativos e sem instalar cliente de banco.

![Comparação: 7 PDVs na versão 7.12.4 e 2 na 7.11.9](docs/img/comparar.png)

## Em um minuto

1. Crie a rede e, em Frota, um perfil de sistema (motor, porta, usuário; a senha vai para o cofre do sistema).
2. Adicione as lojas colando os IPs ou pela regra de endereço.
3. Em Consulta, marque as lojas, escreva o SQL e aperte `Ctrl+Enter`.

O passo a passo completo, com telas, está no [guia de uso](docs/guia.md) e dentro do app, em Como usar.

## Download

Baixe na [página de versões](https://github.com/lucaswotta/selectall/releases):

| Sistema | Arquivo | Instalação |
| --- | --- | --- |
| Windows 10 e 11 | `SelectAll_<versão>_x64-setup.exe` | Executar; instala para o usuário, sem pedir administrador |
| Debian, Ubuntu | `SelectAll_<versão>_amd64.deb` | `sudo apt install ./SelectAll_<versão>_amd64.deb` |
| Outros Linux | `SelectAll_<versão>_amd64.AppImage` | `chmod +x` e executar |

Cada versão traz `SHA256SUMS.txt`. Os instaladores não são assinados; o Windows pode pedir confirmação na primeira execução. No Linux, é preciso WebKitGTK 4.1 (Debian 12, Ubuntu 22.04 ou mais novos) e um cofre Secret Service (GNOME Keyring ou KWallet).

## Segurança

- Senhas só no cofre do sistema operacional (Gerenciador de Credenciais no Windows, Secret Service no Linux). Nada de senha em arquivo, planilha ou exportação.
- Leitura em transação `READ ONLY` no MySQL e no PostgreSQL; no SQL Server, análise do SQL e transação sempre desfeita.
- Parâmetros entram como texto citado no dialeto de cada motor, nunca crus no SQL.
- Auditoria local que só aceita acréscimo, com SQL, valores, lojas e resultado de cada equipamento.
- Recomendado: no perfil de leitura, um usuário de banco só com `SELECT`.

## Configuração e limites

- **Ajustes:** equipamentos da mesma loja ao mesmo tempo (padrão 4; no total, até 16) e limites do histórico (30 dias ou 500 MB).
- **Dados:** `selectall.db` em `%APPDATA%\io.github.lucaswotta.selectall` no Windows e `~/.local/share/io.github.lucaswotta.selectall` no Linux.
- **Limites:** até 1.000 linhas e 4 MB por equipamento; prazo de 30 s por consulta e 3 s para conectar; SQL Server 2008/2012 sem atualização (só TLS 1.0) não é aceito.
- **Fora desta versão:** servidor multiusuário, agendamento, Oracle, interface em outro idioma e macOS.

## Desenvolvimento

Rust estável, Node.js 20+ e, no Linux, `libwebkit2gtk-4.1-dev librsvg2-dev libxdo-dev libssl-dev libdbus-1-dev pkg-config`. No Windows, MSVC e NASM. Docker só para os testes contra bancos reais.

```sh
cd app && npm ci && npx tauri dev
```

Verificações da CI: `cargo fmt --all --check`, `cargo clippy --workspace --all-targets -- -D warnings`, `cargo test --workspace`, `npm run check` em `app/` e `sh scripts/test-{mysql,postgres,sqlserver}.sh`, que sobem bancos descartáveis com senhas aleatórias. `npx tauri build` gera os instaladores em `target/release/bundle/`; um tag `v*` publica a versão com as notas do [CHANGELOG](CHANGELOG.md).

| Caminho | Conteúdo |
| --- | --- |
| `crates/core` | `selectall-core`: frota, SQLite, cofre e execução de SQL, sem interface |
| `app/src-tauri` | Comandos Tauri sobre o núcleo |
| `app/src` | Interface em Svelte 5 e TypeScript |
| `design` | Tokens de cor e tipografia, marca e protótipo |
| `docs` | [Definição do produto](docs/produto.md) e [guia de uso](docs/guia.md) |

## Licença

MIT ou Apache-2.0, à sua escolha. Veja [`LICENSE-MIT`](LICENSE-MIT) e [`LICENSE-APACHE`](LICENSE-APACHE).
