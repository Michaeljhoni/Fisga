# Onde paramos e como continuar

Este arquivo passa o projeto de um assistente (ou de uma pessoa) para outro. Ele resume o que existe,
como trabalhar no repositório e o que vem a seguir. Atualizado em 23/10/2026, com o pacote 2
completo (bandeja, iniciar com a sessão, agendador e várias filas).

## O projeto em uma frase

**LDM** é um gerenciador de downloads para Linux inspirado no Internet Download Manager (IDM),
escrito em Rust do zero (clean-room). Inclui uma extensão para Chrome e Firefox que captura
downloads e vídeos, e uma interface GTK4 com os textos em português.
Repositório privado: <https://github.com/Michaeljhoni/ldm>.

## Quem usa e como

- O dono do projeto desenvolve no **Windows**, mas usa e testa no **Arch Linux com o niri**
  (Wayland), em casa, por acesso remoto. Lá ele atualiza com:
  ```bash
  cd ldm && git pull && scripts/install-dev.sh && ldm stop
  ```
  O `ldm stop` encerra o daemon antigo; o novo sobe sozinho no próximo download.
- Ele só vê o que está no `main` do GitHub. **Tudo precisa ser commitado e enviado.**
- Responda **sempre em português (pt-BR)**. Código, comentários e mensagens de commit ficam em
  inglês, como no resto do repositório.
- Ele dá bastante autonomia ("construa bastante coisa"), mas espera o resultado testado.

## Arquitetura

```
navegador ──Native Messaging──▶ ldm-native-host ──socket Unix──▶ ldmd ──▶ downloads
                                                                  ▲
                                         ldm (CLI) ───────────────┤
                                         ldm-gui (GTK4) ──────────┘
```

| Crate | O que faz |
|---|---|
| `crates/ldm-core` | Motor: sondagem com `Range`, segmentação dinâmica, retomada (`arquivo.ldm-part` + `arquivo.ldm-part.json`) e `RateLimiter` (limite de velocidade). Em `examples/range_server.rs` está o servidor de teste local. |
| `crates/ldm-ipc` | Protocolo JSON por linha no socket `$XDG_RUNTIME_DIR/ldm/ldmd.sock`. Tipos `Request`, `Response`, `DownloadInfo`, `Settings`, `Category`, `MediaFormat`, `Conflict`, mais `existing()` e `unique_name()`. |
| `crates/ldm-daemon` (`ldmd`) | Lista, fila e jobs (`manager.rs`); agendador e suspender/desligar (`manager/scheduler.rs`); yt-dlp para vídeos de sites (`ytdlp.rs`); abre o `ldm-gui --pending` para confirmar um download (`gui.rs`); monitor da área de transferência (`clipboard.rs`); notificações. |
| `crates/ldm-cli` (`ldm`) | `add`, `get`, `list`, `confirm`, `pause`, `resume`, `rm`, `watch`, `settings`, `schedule`, `queue`, `move`, `install-hosts`, `stop`. |
| `crates/ldm-native-host` | Ponte entre a extensão e o `ldmd`. |
| `crates/ldm-gui` (`ldm-gui`, app-id `io.ldm.Ldm`) | GTK4 0.10 (feature `v4_10`), glib 0.21. Janelas: principal (`window.rs`), Novo download (`confirm.rs`), progresso / Download concluído (`progress.rs`), Preferências com abas Geral e Filas (`prefs.rs`, `queues_page`), contagem antes de suspender/desligar (`power.rs`). Bandeja com o crate `ksni` 0.3 (`tray.rs`, roda no runtime tokio do `ipc.rs`); autostart (`autostart.rs`). `app.rs` mantém o estado e decide quando o app fica vivo. |
| `extension/` | Extensão MV3 para Chrome e Firefox. `src/background.js` e `src/content.js` colocam o botão "Baixar" sobre os players; `build.sh` gera `extension/dist/`. |

Dados no Linux:
- lista de downloads: `~/.local/share/ldm/downloads.json`;
- configurações: `~/.config/ldm/settings.json`;
- logs: `~/.local/state/ldm/ldmd.log` e `ldm-gui.log`.

## O que já está pronto (tudo no `main`, com CI verde)

**Motor e daemon**
- Download segmentado com várias conexões, pausar e retomar (inclusive depois de reiniciar), nome
  vindo do `Content-Disposition` e fila com N downloads simultâneos.
- "Baixar agora" fura a fila; "Adicionar à fila" espera uma vaga.
- **Categorias**: Vídeos, Músicas, Imagens, Documentos, Compactados, Programas e Outros, cada uma
  numa subpasta de `~/Downloads` (dá para desligar).
- **Limite de velocidade global**, que muda na hora, inclusive com download em andamento.
- **Vídeos de sites via yt-dlp**:
  - Antes de confirmar, o daemon sonda com `yt-dlp -J`.
  - Um endereço que é uma lista (feed, busca, canal, playlist) é bloqueado.
  - O yt-dlp sempre roda com `--max-downloads 1`; a saída 101 conta como sucesso.
  - Esse bloqueio corrigiu o bug do download sem fim a partir do YouTube.
- **Qualidade do vídeo** (pacote 1): uma opção por resolução, mais "Somente áudio" (`.m4a`), cada
  uma com o tamanho estimado. A escolha vai para o yt-dlp como `--format`.
- **Arquivo já existe** (pacote 1): salvar com outro nome (`arquivo (1).iso`), substituir, ou
  continuar o download incompleto.
- **Captura de links copiados** (pacote 1):
  - Usa o `wl-paste --watch`. A variável `LDM_CLIPBOARD_CMD` troca o comando e é usada nos testes.
  - Só pega links de vídeo, música, programa, arquivo compactado e PDF.
  - Se nenhuma janela puder perguntar, o link é descartado.

- **Várias filas** (pacote 2): `ldm_ipc::Queue { id, name, max_parallel, schedule }`.
  - A fila principal (`MAIN_QUEUE` = 0) continua nos campos antigos `Settings.max_parallel` e
    `Settings.schedule`, para os `settings.json` antigos valerem; as outras ficam em
    `Settings.queues`. Use `all_queues()`, `queue(id)`, `set_queue()` e `remove_queue()`.
  - Cada download tem `DownloadInfo.queue` e `order` (posição; menor começa antes). Downloads
    antigos recebem `order = id` ao carregar.
  - `Manager::schedule` respeita o limite de cada fila. "Baixar agora" ocupa uma vaga da fila mas
    começa mesmo se ela estiver cheia.
  - `Request::SetQueue` (vai para o fim da outra fila) e `Request::Reorder` (subir, descer,
    topo, fim, entre os não concluídos da fila). Excluir uma fila nas configurações manda os
    downloads dela para a principal.
  - O agendador é por fila (`Scheduler { queues }`), e o `scheduler.json` virou um mapa por fila
    (o formato antigo ainda é lido).
  - Uma fila que termina com "suspender/desligar" espera os downloads das outras filas
    (`power_when_idle`: nada baixando nem na fila de uma fila aberta). Desligar vence suspender.
  - A posição (`order`) vem de `Inner::next_order` (maior posição + 1), e os ids de fila nunca são
    reaproveitados (`Settings.last_queue_id`).
- **Agendador** (pacote 2), por fila (antes só em `Settings.schedule`):
  - Horário de início, de parada (opcional) e dias da semana. A janela que passa da meia-noite
    pertence ao dia em que começou (`Schedule::window_at`, testada em `ldm-ipc`).
  - Fora do horário a fila não inicia nada; "Baixar agora" e retomar à mão (`resume` com a fila
    fechada marca `forced`) começam na hora.
  - No fim da janela, só os jobs que a fila iniciou (`Job.from_queue`) são pausados e voltam para
    `Queued` (`Job.requeue`). Ligar o agendador fora do horário não pausa nada.
  - "Fila concluída" só vale numa janela cujo início foi visto (o relógio passou pelo horário,
    ou o usuário mudou o agendamento dentro dela) e depois que um download iniciado pela fila
    terminou. Pausados que a fila iniciou ainda contam como pendentes. Iniciar o `ldmd` no meio
    de uma janela nunca suspende/desliga, e uma janela sem horário de parada fica fechada até o
    próximo início. `started`/`done` ficam em `~/.local/share/ldm/scheduler.json`, para um
    reinício não repetir o desligamento.
  - Ao terminar a fila: nada, suspender ou desligar. O daemon anuncia um `PowerPlan`
    (`Response::Power`), espera 60 s (`LDM_POWER_DELAY`) e roda `systemctl suspend|poweroff`
    (`LDM_POWER_CMD` troca o comando nos testes). Sem GUI conectada, abre o `ldm-gui --pending`
    para mostrar a contagem. `Request::CancelPower` / `ldm schedule --cancel` cancelam.
  - O relógio é conferido a cada 15 s (`LDM_SCHEDULER_TICK_MS`).

- **Atualizar endereço** (pacote 3, `manager/refresh.rs`):
  - `Request::RefreshUrl { id, url }`: sonda o link novo, recusa outro tamanho, troca o link,
    limpa o ETag do `.ldm-part.json` (um link novo costuma vir com outro ETag, e o motor
    recomeçaria do zero) e retoma.
  - `Request::AwaitUrl { id, waiting }` (`DownloadInfo.awaiting_url`): o próximo download
    pendente vindo do navegador com o mesmo tamanho (de preferência o mesmo nome) é absorvido
    em `probe_pending`; o link, os cookies e os cabeçalhos dele vão para o download antigo.
  - `DownloadInfo.page` (página ou referer) alimenta o botão "Abrir a página".
  - O `range_server` ignora a query string, para os testes simularem links novos.

- **Proxy e logins por site** (pacote 3, `manager/network.rs`):
  - `Settings.proxy: ProxySettings { mode: System|None|Manual, url }`. O cliente do `reqwest`
    fica num `Mutex` e é recriado em `set_settings` quando o proxy muda; um proxy inválido é
    recusado antes de salvar. `ldm_core::build_client_with(Proxy::...)`.
  - O yt-dlp recebe o proxy manual pelas variáveis `*_proxy` (não pelo `--proxy`, para a senha
    não aparecer no `/proc/<pid>/cmdline`); "sem proxy" é `--proxy ""`.
  - `SiteLogin { host, user, password }` em `~/.config/ldm/logins.json` (0600), pedidos
    `GetLogins`/`SetLogins`, nunca no `Settings` (que é logado e transmitido). `Manager::headers`
    acrescenta `Authorization: Basic` aos links diretos do host exato (`*.host` cobre os
    subdomínios; o exato vence), só por https ou para o próprio computador, a não ser que o
    navegador já tenha mandado um `Authorization`. O `Debug` de `ProxySettings` e de
    `SiteLogin` esconde as senhas; o `settings.json` passou a ser 0600 (o proxy pode ter senha).
  - `RequestSpec::for_final_url`: o motor baixa da URL final do redirecionamento, e
    `Authorization`/`Cookie` só seguem se for o mesmo host (e nunca de https para http).
  - Um proxy salvo inválido não cai no "do sistema": o cliente usa `UNREACHABLE_PROXY` e os
    downloads falham até corrigir.
  - Não vale para o yt-dlp (o login de sites de vídeo é outro assunto: cookies do navegador).
  - O `range_server` também é um proxy HTTP e tem `/private/files/...` com senha `teste:senha`.

**Extensão**
- Botão sobre os players, menu de contexto e detecção de streams.
- Usa a URL atual do frame (`frameUrlOf`). Isso corrigiu o bug de baixar o vídeo recomendado em
  vez do aberto, que acontecia depois da navegação SPA do YouTube.
- Em feeds, usa o link do card (`linkUrl`/`linkTitle`).
- Por padrão manda `confirm: true`; o popup tem uma opção para desligar.

**Interface (GTK4)**
- **Novo download**: endereço, nome editável, pasta, qualidade, tamanho e o aviso de arquivo
  existente, com os botões Baixar agora, Adicionar à fila e Cancelar.
  - O título é exatamente `Novo download`, para regras de janela do niri.
  - Se nenhuma GUI se conectar em 15 s, o download do navegador vai sozinho para a fila.
- **Progresso**, que vira **Download concluído**, com Abrir, Abrir pasta, Pausar e Fechar.
  O "Abrir" fecha a janela só depois de o app abrir o arquivo.
- **Janela principal**:
  - Barra lateral com Todos, Em andamento, Concluídos e as categorias, com contagens.
  - Lista em tempo real, barra de status e um botão para cada ação.
  - Menu ☰: Retomar todos, Pausar todos, Copiar endereço (Ctrl+C), Remover concluídos,
    Preferências (Ctrl+,) e Sobre.
  - **Clique direito** num download: Abrir, Abrir pasta, Retomar, Pausar, Copiar endereço e Remover.
- **Preferências**: pasta, subpastas por tipo, downloads simultâneos, conexões, janela de
  progresso, limite de velocidade, captura de links copiados, ícone na bandeja e iniciar com a
  sessão. A aba Filas escolhe, cria e exclui filas, e edita nome, downloads ao mesmo tempo e o
  agendador de cada uma (início, parada, dias e o que fazer ao terminar).
- **Filas na GUI**: seção "Filas" na barra lateral (reconstruída em `MainWindow::set_queues`
  quando as configurações mudam), lista ordenada por `order`, e no clique direito "Mover para
  cima/baixo" e "Mover para a fila ▸". A janela Novo download tem o campo Fila.
- Atalho `io.ldm.Ldm.desktop` no menu de aplicativos, com caminho absoluto no `Exec`.
- **Bandeja** (pacote 2): `Settings.tray`, ligada por padrão. O ícone só existe numa instância
  "persistente" (janela principal aberta ou `--background`), nunca no `--pending`. Com o ícone
  ativo, fechar a janela mantém o app (um `hold` do GApplication). Sem bandeja (o `spawn` do
  `ksni` falha), fechar a janela fecha o app, como antes. Menu: resumo, Abrir, Adicionar URL,
  Retomar todos, Pausar todos, Sair (só a interface).
- **Iniciar com a sessão** (pacote 2): `~/.config/autostart/io.ldm.Ldm.desktop` com
  `ldm-gui --background`. Nesse modo o `ksni` usa `assume_sni_available(true)`: o ícone aparece
  quando o waybar/noctalia subir, mesmo que depois do LDM. Com "iniciar com a sessão" ligado e a
  bandeja desligada, o `--background` roda sem nada visível (só confirma os downloads do
  navegador); talvez valha avisar disso nas Preferências.

**Testes** (todos no CI: `.github/workflows/ci.yml`, ubuntu-24.04)
- `cargo test --workspace`: cerca de 70 testes unitários.
- `cargo clippy --workspace --all-targets`.
- `scripts/e2e.sh target/debug`: daemon, CLI e host nativo de verdade contra o `range_server`.
  Cobre pausar e retomar, confirmação, fallback, configurações, limite de velocidade, arquivo
  existente, link copiado e o agendador (fila esperando, abrindo e "suspendendo" no fim).
- `scripts/ci/gui-smoke.sh`: a GUI sob Xvfb com xdotool. Os screenshots ficam no artifact
  `gui-screenshots` de cada execução. Inclui uma segunda fila, a aba Filas e o `ldm-gui --background`
  confirmando um download sem abrir outro `--pending`. Não há bandeja no Xvfb, então o ícone
  em si só é testado de verdade no Arch.
- `extension/test/real-clicks.mjs`: Chrome de verdade via CDP pela pipe, com native messaging
  simulado. Roda manualmente, no Windows, com `CHROME=...`.

O dono testou no Arch: 5 downloads do YouTube corretos, o pacote 1 (ok em 22/10) e a bandeja,
a sessão e o agendador do pacote 2 (ok em 23/10). **Ainda não testou as várias filas.**

## Como trabalhar no repositório

### Fluxo de commits
1. Crie um branch `wip/<assunto>` a partir do `main`.
2. Commit e `git push -u origin wip/<assunto>`.
3. Espere o CI:
   ```bash
   gh run list --branch wip/<assunto>
   gh run watch <id> --exit-status
   ```
4. Com o CI verde, faça fast-forward:
   ```bash
   git checkout main && git merge --ff-only wip/<assunto> && git push
   ```
   Depois apague o branch, local e remoto.
5. Para conferir a GUI: `gh run download <id>` e veja os PNGs.

Mudanças só de documentação podem ir direto para o `main`.

### Compilar no Windows (não há Rust nativo aqui)
- Use Docker: `scripts/docker-cargo.sh test --workspace` (ou `clippy ...`). A imagem `ldm-build` é
  rust:1-slim-trixie com GTK4.
- Para rodar o e2e no container:
  ```bash
  root="$(pwd -W)"; MSYS_NO_PATHCONV=1 docker run --rm -v "$root:/src" -w /src \
    -v ldm-cargo-registry:/usr/local/cargo/registry -v ldm-target:/target -e CARGO_TARGET_DIR=/target \
    ldm-build bash -c "cargo build -q --workspace --bins --examples && bash scripts/e2e.sh /target/debug"
  ```
- **Docker Desktop costuma travar aqui.** O erro aparece no log como
  `listening on unix://.../Docker/run/dockerInference`: sobra um socket antigo que não se apaga.
  Correção, com o Docker fechado:
  1. Renomeie `%LOCALAPPDATA%\Docker\run` e `%LOCALAPPDATA%\docker-secrets-engine` para
     `*.old-<data>`.
  2. Crie as duas pastas de novo, vazias.
  3. Abra o Docker Desktop e não o mate durante a inicialização.

  Se a memória acabar, procure vazamentos (já foi o `SteelSeriesCaptureSvc`). Se nada resolver,
  envie o branch e deixe o CI compilar.

### Armadilhas conhecidas
- No Bash desta máquina, heredocs com aspas às vezes quebram (há um hook "rtk" que reescreve
  comandos). Para patches grandes, escreva um script Python num arquivo e rode-o.
- O `.gitattributes` força LF nos `*.sh`; sem isso o checkout no Windows quebra os scripts.
- No `e2e.sh`, use `last_id` em vez de ids fixos nos passos novos, e arquivos de teste com
  tamanhos que ainda não foram usados (`/files/<bytes>.bin` no `range_server`).
- No glib 0.21 não existe `@strong` no `clone!`: use clones manuais.
- Ao chamar `GtkWindowExt::set_focus`, qualifique o trait.
- Clippy novo (rust 1.99) reclama de `filter().next_back()` e de `map` que deveria ser `inspect`.

## Próximos passos (em ordem de prioridade)

O pacote 2 está completo e testado no Arch (23/10). No pacote 3, o "Atualizar endereço" está
pronto; peça ao dono para testar: pausar um download, clique direito → Atualizar endereço… →
Capturar do navegador, clicar no link de novo na página, e ver o download continuar.

### Pacote 3: rede e casos especiais
- ~~Atualizar o endereço~~ (feito e testado no Arch).
- ~~Proxy HTTP/SOCKS e login por site~~ (feito e testado no Arch).
- ~~Limite de velocidade por download~~ (feito): `DownloadInfo.speed_limit`,
  `Request::SetSpeedLimit`, um `RateLimiter` por `Job`. O motor recebe uma lista de
  limitadores (`DownloadOptions::limiters`: o global e o do download) e respeita todos; o
  yt-dlp recebe o menor (`combined_limit`) e só no próximo início. Na GUI, fica na janela de
  progresso ("No máximo ... KB/s"); "Limitar velocidade…" no clique direito abre essa janela.
- ~~Download em lote~~ (feito): `ldm_ipc::expand_batch` (um `*`, números com zeros à
  esquerda ou letras, no máximo `MAX_BATCH`) e `parse_url_list`; `Request::AddBatch` confere
  todos os endereços antes e cria tudo com um salvamento só (`Manager::insert` sem salvar,
  usado por `add` e `add_batch`); sem a janela Novo download. GUI em `batch.rs`.
- HLS/DASH sem yt-dlp, e FTP.

### Pacote 4: acabamento
- ~~Busca na lista, colunas configuráveis, exportar e importar a lista~~ (feito):
  `SearchEntry` no lugar do título (`format::matches_search`; enquanto ela tem foco, os atalhos
  Delete e Ctrl+C da janela ficam desligados), `window::COLUMNS` com ações `win.column-<id>` e
  `Settings.hidden_columns`, coluna "Adicionado em" (`format::added`). Exportar = os endereços
  da vista atual num `.txt`; importar abre esse texto na janela de lote. CLI: `ldm export`.
- ~~Som, comando e ClamAV ao concluir~~ (feito, `manager/complete.rs`, `Settings.on_complete`):
  `after_complete` roda depois de `finish` (também para os vídeos do yt-dlp): ClamAV (infectado
  = `info.error` num download concluído, e para aí), o comando do usuário em `sh -c` com
  `$LDM_FILE`... no ambiente (nunca no texto), e o som. Nos testes, `LDM_CLAMSCAN_CMD` e
  `LDM_SOUND_CMD` trocam o ClamAV e o tocador.
- ~~Grabber~~ (feito, `manager/grab.rs`): `Request::Grab { url, depth }` lê a página e, até
  `depth` (máximo `MAX_GRAB_DEPTH` = 3), as páginas do mesmo host abaixo da pasta inicial;
  `extract_links` pega `href`/`src` sem parser de HTML; limites de 100 páginas, 5 MB e 15 s por
  página. Os filtros (`grab_filter`, `glob_matches`) ficam no `ldm-ipc`, usados pela GUI
  (`grabber.rs`) e pela CLI (`ldm grab`); adicionar é um `AddBatch`. Ainda sem filtro por
  tamanho (precisaria de uma sondagem por arquivo).
- ~~Pacote para o Arch~~ (feito): `packaging/arch/PKGBUILD` (`ldm-git`), fonte em `$LDM_GIT`
  (o checkout local enquanto o repositório é privado). `ldm install-hosts --system $pkgdir
  --host-path /usr/bin/ldm-native-host` grava os manifestos do sistema. O job `arch-package`
  do CI roda o `makepkg` num container `archlinux`, instala o pacote e confere os arquivos.
  Para publicar no AUR: tornar o repositório público, gerar o `.SRCINFO`
  (`makepkg --printsrcinfo`) e escolher uma licença (hoje `custom`, sem arquivo LICENSE).
- Flatpak, .deb e .rpm.

A análise do IDM que originou o roadmap está em `docs/ANALISE-IDM.md`, com o catálogo de
funcionalidades e a arquitetura. O README tem o uso, a regra do niri e a seção de problemas.
