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
| `crates/ldm-gui` | `ldm-gui` | Interface GTK4: janela "Novo download" e lista de downloads (só cliente do `ldmd`) |
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
Opcionais: `yt-dlp` e `ffmpeg` (vídeos de sites), `libnotify` (notificações).

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

- **Janela principal** (`ldm-gui`, ou "LDM" no menu de aplicativos): lista com nome, tamanho,
  progresso, velocidade, tempo restante e estado, atualizada em tempo real. Botões para adicionar
  URL (Ctrl+N; sugere o link da área de transferência), retomar, pausar, remover (Delete; pergunta se
  apaga também o arquivo), abrir o arquivo e abrir a pasta. Clique duplo abre um arquivo concluído
  ou a confirmação de um download pendente.
- **Novo download**: aparece para cada download vindo do navegador, com endereço, nome (editável),
  pasta (botão "Alterar…") e tamanho. **Baixar agora** começa na hora, mesmo com a fila cheia;
  **Adicionar à fila** espera uma vaga (3 simultâneos); **Cancelar**, Esc ou fechar a janela descarta.
  Enter = Baixar agora.

A GUI é só um cliente do `ldmd`: fechar a janela não interrompe os downloads.

### Do clique no navegador à janela de confirmação

1. A extensão manda `{"type":"download", …, "confirm": true}` ao `ldm-native-host` (o popup tem a
   opção **"Confirmar cada download numa janela do LDM"**; desmarcada, envia `"confirm": false` e o
   download começa direto, como antes). Mensagens sem o campo também pedem confirmação.
2. O host inicia o `ldmd` se preciso e envia `Request::Add` com `confirm: true`; responde à extensão
   `{"type":"added","id":…,"pending":true}`, e o botão mostra "Confirme no LDM".
3. O `ldmd` cria o download no estado **pendente** (`State::Pending`, "aguardando" no `ldm list`) e,
   para links diretos, sonda o servidor para descobrir tamanho e nome. A mudança vai para todos os
   assinantes (`Request::Subscribe`).
4. Se há uma GUI conectada (assinante com `confirmer: true`), ela abre a janela **Novo download**.
   Senão, o `ldmd` executa `ldm-gui --pending`, que mostra só essa janela e fecha depois da resposta.
5. A janela responde com `Request::Confirm { id, now, file_name, output_dir }` (Baixar agora /
   Adicionar à fila) ou `Request::Remove` (Cancelar).
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
ldm list
ldm pause 3 && ldm resume 3
ldm rm 3 --delete-file
ldm watch                                        # mudanças em tempo real
```

## Desenvolvimento

```bash
cargo test --workspace          # testes (incluem um servidor HTTP local com Range)
cargo build --workspace --examples && scripts/e2e.sh   # ponta a ponta: daemon, CLI, host nativo e confirmação (servidor local)
node extension/tools/make-icons.mjs
node scripts/dev-server.mjs     # abra http://localhost:8787/extension/test/overlay.html
```

No Windows/macOS dá para compilar e testar dentro de um container Linux: `scripts/docker-cargo.sh test --workspace`.
O script monta, na primeira vez, a imagem `ldm-build` (`scripts/Dockerfile.build`: imagem oficial do
Rust em Debian trixie + `libgtk-4-dev`).

### Comportamento no niri / Wayland

O botão flutuante é desenhado dentro da página pelo navegador, então funciona igual em qualquer
compositor (niri, Hyprland, Sway, GNOME, KDE) e no X11.

A GUI usa GTK4 com `app-id` fixo `io.ldm.Ldm`, e a janela de confirmação tem sempre o título exato
`Novo download`. Num compositor *tiling*, ela abriria como uma coluna inteira; para abrir flutuante e
centralizada, como no IDM, adicione ao `~/.config/niri/config.kdl`:

```kdl
window-rule {
    match app-id=r#"^io\.ldm\.Ldm$"# title="^Novo download$"
    open-floating true
    open-focused true
}
```

(Quando a janela principal está aberta, a confirmação já nasce como diálogo dela, e o niri a deixa
flutuante sozinho.) Equivalentes: Sway, `for_window [app_id="io.ldm.Ldm" title="^Novo download$"]
floating enable`; Hyprland, `windowrulev2 = float, class:^(io\.ldm\.Ldm)$, title:^(Novo download)$`.

## Estado

- [x] Motor segmentado com retomada, testado
- [x] Daemon, CLI, host nativo
- [x] Extensão: botão sobre vídeo/áudio, menu de mídias detectadas, captura de downloads, menu de contexto
- [x] Interface gráfica (GTK4): janela "Novo download" ao clicar no botão, lista com ações
- [ ] Categorias, configurações na GUI, ícone na bandeja
- [ ] Limite de velocidade, agendador, categorias por tipo de arquivo
- [ ] HLS/DASH nativo (hoje via yt-dlp), FTP
- [ ] Pacotes (AUR, Flatpak, .deb/.rpm)
