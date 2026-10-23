# LDM — gerenciador de downloads para Linux

> Nome provisório. Inspirado no comportamento do Internet Download Manager, mas escrito do zero
> (veja [docs/ANALISE-IDM.md](docs/ANALISE-IDM.md)).

O destaque é a **extensão do navegador**: ao passar o mouse sobre um vídeo ou música em qualquer
site, aparece um botão **Baixar** no canto do player. Um clique abre a janela **Novo download** do
LDM, como no IDM, para confirmar nome, pasta e tamanho; a setinha ▾ mostra tudo o que foi detectado
na página (stream HLS/DASH, arquivos MP4/MP3, ou "detectar automaticamente", que usa o yt-dlp em
sites como YouTube).

## Peças

| Pasta | Binário | O que faz |
|---|---|---|
| `crates/ldm-core` | — | Motor: várias conexões por arquivo, divisão dinâmica de segmentos, retomada |
| `crates/ldm-daemon` | `ldmd` | Serviço em segundo plano: lista, fila (3 simultâneos), notificações, yt-dlp |
| `crates/ldm-gui` | `ldm-gui` | Interface GTK4: janela "Novo download", lista de downloads e ícone na bandeja (só cliente do `ldmd`) |
| `crates/ldm-cli` | `ldm` | Linha de comando (`add`, `get`, `list`, `confirm`, `pause`, `resume`, `rm`, `watch`, `install-hosts`) |
| `crates/ldm-native-host` | `ldm-native-host` | Ponte entre a extensão (Native Messaging) e o `ldmd` |
| `crates/ldm-ipc` | — | Mensagens JSON pelo socket `$XDG_RUNTIME_DIR/ldm/ldmd.sock` |
| `extension/` | — | Extensão MV3 para Chrome/Chromium/Brave/Edge/Vivaldi e Firefox |

```
navegador ──Native Messaging──▶ ldm-native-host ──Unix socket──▶ ldmd ──▶ ldm-core / yt-dlp
                                              ldm (CLI) ──Unix socket──┤
                                          ldm-gui (GTK4) ──Unix socket──┘
```

## Instalar (qualquer distro)

Requisitos: Rust e GTK4 ≥ 4.10 com os cabeçalhos (Arch: `sudo pacman -S rust gtk4`; Debian 13/Ubuntu
24.04: `libgtk-4-dev` + [rustup](https://rustup.rs); Fedora: `gtk4-devel`).
Opcionais: `yt-dlp` e `ffmpeg` (vídeos de sites), `libnotify` (notificações), `wl-clipboard`
(capturar links copiados no Wayland).

```bash
scripts/install-dev.sh
```

Isso compila, instala `ldm`, `ldmd`, `ldm-native-host` e `ldm-gui` em `~/.local/bin` (e o atalho
`io.ldm.Ldm.desktop` no menu de aplicativos), registra o host nativo em todos os navegadores
encontrados e gera a extensão em `extension/dist/`. Depois carregue a extensão:

- **Chromium e derivados:** `chrome://extensions` → Modo do desenvolvedor → Carregar sem compactação →
  `extension/dist/chrome`. O ID fica fixo (`ocifdbhjaijkdgcnpigjlemkmahkojcm`) graças à chave no manifesto.
- **Firefox:** `about:debugging#/runtime/this-firefox` → Carregar extensão temporária →
  `extension/dist/firefox/manifest.json`. No popup da extensão, clique em "Permitir acesso a todos os
  sites" (o Firefox trata essa permissão como opcional). Para instalar de forma permanente, a extensão
  precisa ser assinada pela Mozilla (ou use Firefox Developer Edition/Nightly/LibreWolf com
  `xpinstall.signatures.required=false`).

Navegadores em Flatpak/Snap rodam isolados e não conseguem iniciar o host nativo sem configuração extra.

## Interface gráfica (`ldm-gui`)

- **Janela principal** (`ldm-gui`, ou "LDM" no menu de aplicativos): à esquerda, como no IDM,
  *Todos os downloads*, *Em andamento*, *Concluídos* e as **categorias** (Vídeos, Músicas, Imagens,
  Documentos, Compactados, Programas, Outros), cada uma com a contagem. À direita, a lista com nome,
  tamanho, progresso, velocidade, tempo restante e estado, atualizada em tempo real. Botões para
  adicionar URL (Ctrl+N; sugere o link da área de transferência), retomar, pausar, remover (Delete;
  pergunta se apaga também o arquivo), abrir o arquivo e abrir a pasta. No menu (☰): *Retomar
  todos*, *Pausar todos*, *Copiar endereço* (Ctrl+C), *Remover concluídos da lista*,
  *Preferências* (Ctrl+,) e *Sobre*. Clique duplo: confirma um download
  pendente, abre um concluído ou mostra o progresso dos demais. **Clique direito** num download:
  menu com *Abrir*, *Abrir pasta*, *Retomar*, *Pausar*, *Copiar endereço* e *Remover…*.
- **Novo download**: aparece para cada download vindo do navegador, com endereço, nome (editável),
  pasta (botão "Alterar…") e tamanho. **Baixar agora** começa na hora, mesmo com a fila cheia;
  **Adicionar à fila** espera uma vaga; **Cancelar**, Esc ou fechar a janela descarta. Enter = Baixar
  agora. Se o endereço for uma lista de vídeos (página inicial do YouTube, busca, canal, playlist), a
  janela avisa e só deixa cancelar.
  - **Qualidade** (vídeos de sites): lista as resoluções que o vídeo tem (1080p, 720p, 480p...) e
    *Somente áudio*, cada uma com o tamanho estimado. A primeira é a melhor; *Somente áudio* baixa
    um `.m4a` e vai para a pasta Músicas.
  - **Já existe**: se a pasta já tem um arquivo com esse nome, a janela pergunta o que fazer:
    salvar com outro nome (`arquivo (1).iso`, o padrão) ou substituir o existente. Se lá estiver um
    download incompleto, a opção padrão é **continuar de onde parou**; dá também para recomeçar do zero.
- **Links copiados** (como o monitor da área de transferência do IDM): ao copiar o endereço de um
  vídeo, música, programa, arquivo compactado ou PDF em qualquer aplicativo, a janela *Novo
  download* abre. Páginas, imagens e textos são ignorados. No Wayland usa o `wl-paste --watch`
  (pacote `wl-clipboard`; funciona no niri, Sway, Hyprland e KDE). Desligue em Preferências →
  "Capturar links copiados" ou com `ldm settings --watch-clipboard false`. Se a janela não puder
  abrir, o link copiado é descartado (não vira download sozinho).
- **Progresso** (a janela de status do IDM): abre depois de "Baixar agora", com barra, quanto já
  baixou, velocidade, tempo restante, se dá para retomar, a pasta e **Pausar/Retomar**. Ao terminar
  vira **Download concluído**, com **Abrir** e **Abrir pasta**. Pode ser desligada nas Preferências.
- **Preferências** (aba *Geral*): pasta de downloads, subpastas por tipo de arquivo, downloads ao mesmo tempo,
  conexões por arquivo, a janela de progresso, capturar links copiados e o **limite de velocidade** (KB/s para todos os
  downloads juntos; muda na hora, até nos que estão baixando, e aparece na barra de status). Ficam
  no daemon, em `~/.config/ldm/settings.json`. Vídeos de sites (yt-dlp) recebem o limite por
  processo (`--limit-rate`). Também ali: **ícone na bandeja** e **iniciar com a sessão** (veja abaixo).
- **Bandeja**: com a opção ligada (padrão), o LDM mostra um ícone na bandeja do sistema
  (StatusNotifierItem: waybar, noctalia, KDE, GNOME com a extensão AppIndicator). Fechar a janela
  principal deixa o LDM ali, pronto para confirmar os downloads do navegador. Clique no ícone para
  abrir a janela; o menu tem o resumo (quantos baixando, velocidade, fila), *Adicionar URL…*,
  *Retomar todos*, *Pausar todos* e *Sair* (só fecha a interface; os downloads continuam no `ldmd`).
  Sem bandeja disponível, fechar a janela fecha o LDM, como antes. No waybar, ative o módulo `tray`.
- **Iniciar com a sessão**: cria `~/.config/autostart/io.ldm.Ldm.desktop`, que roda
  `ldm-gui --background` ao entrar: sem janela, só o ícone (que aparece quando a bandeja subir). O
  GNOME e o KDE leem essa pasta; no niri, Sway e Hyprland ela vale quando a sessão é iniciada pelo
  systemd (o `niri-session`, por exemplo). Se não funcionar, adicione ao `~/.config/niri/config.kdl`:
  `spawn-at-startup "ldm-gui" "--background"`.
- **Agendador** (aba *Agendador*, como o *Scheduler* do IDM): a fila só roda das HH:MM às HH:MM nos
  dias escolhidos. Fora desse horário os downloads ficam *na fila*; *Baixar agora* e retomar um
  download à mão começam na hora mesmo assim. No horário de parada, os downloads da fila voltam
  para ela e continuam na próxima vez. Sem horário de parada, a fila vai até acabar. **Ao terminar
  a fila**: não fazer nada, suspender ou desligar o computador (`systemctl suspend`/`poweroff`), com
  uma janela que conta 60 segundos e deixa cancelar (ou `ldm schedule --cancel`). Isso só acontece
  quando a fila começou no horário (ou o agendador foi configurado dentro dele) e pelo menos um
  download terminou; ligar o computador no meio do horário nunca o desliga sozinho.

### Categorias e pastas

Cada download recebe uma categoria pela extensão do arquivo (vídeos de sites via yt-dlp são
*Vídeos*, a não ser que o arquivo seja só áudio). Como no IDM, por padrão cada categoria vai para
uma subpasta da pasta de downloads: `~/Downloads/Vídeos`, `Músicas`, `Imagens`, `Documentos`,
`Compactados` (zip, rar, 7z, tar.*) e `Programas` (AppImage, deb, rpm, flatpak, iso, exe...);
o que não se encaixa fica em `~/Downloads`. A pasta escolhida na janela "Novo download" ou com
`-o` no terminal sempre vale. Para tudo na mesma pasta: desligue "Separar por tipo de arquivo" nas
Preferências, ou `ldm settings --category-folders false`.

A GUI é só um cliente do `ldmd`: fechar a janela não interrompe os downloads.

### Do clique no navegador à janela de confirmação

1. A extensão manda `{"type":"download", …, "confirm": true}` ao `ldm-native-host` (o popup tem a
   opção **"Confirmar cada download numa janela do LDM"**; desmarcada, envia `"confirm": false` e o
   download começa direto, como antes). Mensagens sem o campo também pedem confirmação.
2. O host inicia o `ldmd` se preciso e envia `Request::Add` com `confirm: true`; responde à extensão
   `{"type":"added","id":…,"pending":true}`, e o botão mostra "Confirme no LDM".
3. O `ldmd` cria o download no estado **pendente** (`State::Pending`, "aguardando" no `ldm list`) e
   descobre tamanho e nome: links diretos com uma requisição `Range`, vídeos de sites com
   `yt-dlp -J`. Se o yt-dlp disser que o endereço é uma lista (feed, busca, canal, playlist), o
   download fica marcado com erro e não pode ser confirmado. A mudança vai para todos os assinantes
   (`Request::Subscribe`).
4. Se há uma GUI conectada (assinante com `confirmer: true`), ela abre a janela **Novo download**.
   Senão, o `ldmd` executa `ldm-gui --pending`, que mostra só essa janela e fecha depois da resposta.
5. A janela responde com `Request::Confirm { id, now, file_name, output_dir, format, conflict }`
   (Baixar agora / Adicionar à fila; `format` é a qualidade escolhida, `conflict` o que fazer com um
   arquivo que já existe: `rename`, `overwrite` ou `resume`) ou `Request::Remove` (Cancelar).
6. Se o `ldm-gui` não existir, ou não se conectar em 15 s (sem display, por exemplo), o download vai
   para a fila sozinho, com uma notificação: um clique no navegador nunca se perde.

O `ldmd` herda o ambiente de quem o iniciou (normalmente o navegador, com `WAYLAND_DISPLAY`), e é
esse ambiente que o `ldm-gui` recebe. `LDM_GUI=/caminho/ldm-gui` troca o executável; `LDM_GUI=`
(vazio) desliga a janela e todo download do navegador vai direto para a fila.

## Uso pela linha de comando

```bash
ldm add https://exemplo.com/arquivo.iso --wait   # envia ao daemon e acompanha
ldm get https://exemplo.com/arquivo.iso -c 16    # baixa direto, sem daemon (Ctrl+C pausa)
ldm add https://exemplo.com/arquivo.iso --confirm # abre a janela "Novo download"
ldm confirm 4 --queue -o ~/ISOs                   # responde a um download pendente pelo terminal
ldm confirm 5 --quality 720                      # vídeo de site em 720p (ou --quality audio)
ldm confirm 6 --if-exists overwrite              # arquivo já existe: rename, overwrite ou resume
ldm list
ldm pause 3 && ldm resume 3
ldm rm 3 --delete-file
ldm watch                                        # mudanças em tempo real
ldm settings                                     # configurações (pasta, subpastas, fila, conexões)
ldm settings --category-folders false --max-parallel 5
ldm settings --speed-limit 2048                  # 2 MB/s para tudo (0 = sem limite)
ldm settings --watch-clipboard false             # não abrir a janela para links copiados
ldm settings --tray false                        # sem ícone na bandeja
ldm schedule --on --start 02:00 --stop 08:00     # a fila só roda de madrugada
ldm schedule --days seg,ter,qua,qui,sex --when-done desligar
ldm schedule --stop nenhum                       # vai até terminar a fila
ldm schedule --cancel                            # cancela o desligamento que está contando
ldm schedule --off
```

## Desenvolvimento

```bash
cargo test --workspace          # testes (incluem um servidor HTTP local com Range)
cargo build --workspace --bins --examples && scripts/e2e.sh   # ponta a ponta: daemon, CLI, host nativo e confirmação (servidor local)
node extension/tools/make-icons.mjs
node scripts/dev-server.mjs     # abra http://localhost:8787/extension/test/overlay.html
```

No Windows/macOS dá para compilar e testar dentro de um container Linux: `scripts/docker-cargo.sh test --workspace`.
O script monta, na primeira vez, a imagem `ldm-build` (`scripts/Dockerfile.build`: imagem oficial do
Rust em Debian trixie + `libgtk-4-dev`).

### Comportamento no niri / Wayland

O botão flutuante é desenhado dentro da página pelo navegador, então funciona igual em qualquer
compositor (niri, Hyprland, Sway, GNOME, KDE) e no X11.

A GUI usa GTK4 com `app-id` fixo `io.ldm.Ldm`. A janela principal se chama sempre `LDM`; as outras
(*Novo download*, a de progresso, *Download concluído*, *Preferências*) são diálogos. Num compositor
*tiling* eles abririam como colunas inteiras; para abrirem flutuantes, como no IDM, adicione ao
`~/.config/niri/config.kdl`:

```kdl
window-rule {
    match app-id=r#"^io\.ldm\.Ldm$"#
    exclude title="^LDM$"
    open-floating true
    open-focused true
}
```

(Quando a janela principal está aberta, os diálogos já nascem ligados a ela e o niri os deixa
flutuantes sozinho; a regra vale para quando só o diálogo aparece, ao clicar no navegador.)
Equivalentes: Sway, `for_window [app_id="io.ldm.Ldm" title="^(?!LDM$)"] floating enable`;
Hyprland, `windowrulev2 = float, class:^(io\.ldm\.Ldm)$, title:negative:^LDM$`.

### Problemas

- **Parar tudo:** `ldm stop` pausa todos os downloads e encerra o daemon (`ldm list` / `ldm pause N`
  para um só).
- **Logs:** `~/.local/state/ldm/ldmd.log` (daemon) e `~/.local/state/ldm/ldm-gui.log` (interface,
  quando aberta pelo menu ou pelo daemon).
- **"LDM" no menu não abre nada:** rode `ldm-gui` num terminal para ver o erro; reinstale com
  `scripts/install-dev.sh`, que grava o atalho com o caminho completo do executável.

## Estado

- [x] Motor segmentado com retomada, testado
- [x] Daemon, CLI, host nativo
- [x] Extensão: botão sobre vídeo/áudio, menu de mídias detectadas, captura de downloads, menu de contexto
- [x] Interface gráfica (GTK4): janela "Novo download" ao clicar no botão, lista com ações
- [x] Janela de progresso e de download concluído, categorias por tipo de arquivo (barra lateral e
      subpastas), Preferências
- [x] Limite de velocidade global (muda na hora)
- [x] Ícone na bandeja, iniciar com a sessão, agendador (suspender/desligar ao terminar)
- [ ] Várias filas
- [ ] HLS/DASH nativo (hoje via yt-dlp), FTP
- [ ] Pacotes (AUR, Flatpak, .deb/.rpm)
