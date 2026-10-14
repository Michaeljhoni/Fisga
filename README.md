# LDM — gerenciador de downloads para Linux

> Nome provisório. Inspirado no comportamento do Internet Download Manager, mas escrito do zero
> (veja [docs/ANALISE-IDM.md](docs/ANALISE-IDM.md)).

O destaque é a **extensão do navegador**: ao passar o mouse sobre um vídeo ou música em qualquer
site, aparece um botão **Baixar** no canto do player. Um clique envia a mídia para o LDM; a setinha
▾ mostra tudo o que foi detectado na página (stream HLS/DASH, arquivos MP4/MP3, ou "detectar
automaticamente", que usa o yt-dlp em sites como YouTube).

## Peças

| Pasta | Binário | O que faz |
|---|---|---|
| `crates/ldm-core` | — | Motor: várias conexões por arquivo, divisão dinâmica de segmentos, retomada |
| `crates/ldm-daemon` | `ldmd` | Serviço em segundo plano: lista, fila (3 simultâneos), notificações, yt-dlp |
| `crates/ldm-cli` | `ldm` | Linha de comando (`add`, `get`, `list`, `pause`, `resume`, `rm`, `watch`, `install-hosts`) |
| `crates/ldm-native-host` | `ldm-native-host` | Ponte entre a extensão (Native Messaging) e o `ldmd` |
| `crates/ldm-ipc` | — | Mensagens JSON pelo socket `$XDG_RUNTIME_DIR/ldm/ldmd.sock` |
| `extension/` | — | Extensão MV3 para Chrome/Chromium/Brave/Edge/Vivaldi e Firefox |

```
navegador ──Native Messaging──▶ ldm-native-host ──Unix socket──▶ ldmd ──▶ ldm-core / yt-dlp
                                              ldm (CLI) ──Unix socket──┘
```

## Instalar (qualquer distro)

Requisitos: Rust (Arch: `sudo pacman -S rust`; outras: [rustup](https://rustup.rs)).
Opcionais: `yt-dlp` e `ffmpeg` (vídeos de sites), `libnotify` (notificações).

```bash
scripts/install-dev.sh
```

Isso compila, instala `ldm`, `ldmd` e `ldm-native-host` em `~/.local/bin`, registra o host nativo em
todos os navegadores encontrados e gera a extensão em `extension/dist/`. Depois carregue a extensão:

- **Chromium e derivados:** `chrome://extensions` → Modo do desenvolvedor → Carregar sem compactação →
  `extension/dist/chrome`. O ID fica fixo (`ocifdbhjaijkdgcnpigjlemkmahkojcm`) graças à chave no manifesto.
- **Firefox:** `about:debugging#/runtime/this-firefox` → Carregar extensão temporária →
  `extension/dist/firefox/manifest.json`. No popup da extensão, clique em "Permitir acesso a todos os
  sites" (o Firefox trata essa permissão como opcional). Para instalar de forma permanente, a extensão
  precisa ser assinada pela Mozilla (ou use Firefox Developer Edition/Nightly/LibreWolf com
  `xpinstall.signatures.required=false`).

Navegadores em Flatpak/Snap rodam isolados e não conseguem iniciar o host nativo sem configuração extra.

## Uso pela linha de comando

```bash
ldm add https://exemplo.com/arquivo.iso --wait   # envia ao daemon e acompanha
ldm get https://exemplo.com/arquivo.iso -c 16    # baixa direto, sem daemon (Ctrl+C pausa)
ldm list
ldm pause 3 && ldm resume 3
ldm rm 3 --delete-file
ldm watch                                        # mudanças em tempo real
```

## Desenvolvimento

```bash
cargo test --workspace          # testes (incluem um servidor HTTP local com Range)
scripts/e2e.sh                  # teste ponta a ponta do daemon/CLI/host (usa a internet)
node extension/tools/make-icons.mjs
node scripts/dev-server.mjs     # abra http://localhost:8787/extension/test/overlay.html
```

No Windows/macOS dá para compilar e testar dentro de um container Linux: `scripts/docker-cargo.sh test --workspace`.

### Comportamento no niri / Wayland

O botão flutuante é desenhado dentro da página pelo navegador, então funciona igual em qualquer
compositor (niri, Hyprland, Sway, GNOME, KDE) e no X11. A interface gráfica (próxima etapa) usará
GTK4 com `app-id` fixo, para permitir regras como `open-floating true` no niri.

## Estado

- [x] Motor segmentado com retomada, testado
- [x] Daemon, CLI, host nativo
- [x] Extensão: botão sobre vídeo/áudio, menu de mídias detectadas, captura de downloads, menu de contexto
- [ ] Interface gráfica (GTK4): janela de "novo download" ao clicar no botão, lista, categorias
- [ ] Limite de velocidade, agendador, categorias por tipo de arquivo
- [ ] HLS/DASH nativo (hoje via yt-dlp), FTP
- [ ] Pacotes (AUR, Flatpak, .deb/.rpm)
