# Design do SelectAll

Fase 1: identidade e protótipo navegável, validados antes de qualquer código de interface.

| Arquivo | Conteúdo |
| --- | --- |
| `tokens.css` | Cores, tipografia, espaçamento e raios para tema claro e escuro |
| `marca.html` | Símbolo, paleta, status, tipografia e voz |
| `marca/simbolo.svg` | Asterisco em `currentColor` |
| `marca/icone-app.svg` | Ícone do aplicativo |
| `prototipo.html` | Telas de consulta, correção, frota, consultas salvas, histórico e auditoria com dados fictícios |

Para ver, sirva a pasta por HTTP (o protótipo carrega `tokens.css` e as fontes):

```sh
python3 -m http.server 8765 --directory design
```

Abra `http://localhost:8765/prototipo.html` e `http://localhost:8765/marca.html`.

## Decisões

- Tipografia: IBM Plex Sans e IBM Plex Mono (OFL). Acentuação completa, números tabulares e par sans/mono da mesma família. No aplicativo as fontes serão empacotadas.
- Contraste: texto e status com pelo menos 4,5:1 sobre fundo e superfície nos dois temas.
- Status sempre acompanha texto. Offline é cinza, não vermelho.
- Dados do protótipo são fictícios: rede "Supermercados Aurora", sub-redes `10.{loja}.3.0/24`.
