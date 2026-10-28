<p align="center">
  <img src="crates/fisga-gui/data/io.fisga.Fisga.svg" width="96" alt="">
</p>

<h1 align="center">Fisga</h1>

<p align="center">
  Gerenciador de downloads livre para Linux, Windows e macOS, inspirado no IDM.<br>
  Várias conexões por arquivo, filas, agendador e uma extensão que fisga downloads e vídeos do navegador.
</p>

<p align="center">
  <a href="https://github.com/Michaeljhoni/Fisga/releases/latest"><b>Baixar</b></a> ·
  <a href="#instalar">Instalar</a> ·
  <a href="docs/GUIA.md">Guia</a> ·
  <a href="#in-english">English</a>
</p>

![Janela principal do Fisga](docs/screenshots/janela-principal.png)

## O que faz

- **Rápido:** divide cada arquivo em até 32 conexões e continua de onde parou, mesmo depois de
  desligar o computador.
- **Extensão para Chrome e Firefox:** pega os downloads do navegador e põe um botão **Baixar** sobre
  vídeos e músicas de qualquer site (YouTube e outros, pelo yt-dlp). Alt+clique num link manda o
  link direto para o Fisga.
- **Janela "Novo download"**, como no IDM: confira nome, pasta e fila antes de começar.
- **Filas e agendador:** quantos downloads ao mesmo tempo, horário para começar e parar, suspender
  ou desligar ao terminar.
- **Organizado:** categorias por tipo (Vídeos, Músicas, Programas...), cada uma na sua pasta.
- **E mais:** limite de velocidade, proxy, login por site, soma SHA-256, antivírus e comando ao
  concluir, ícone na bandeja, captura de links copiados.

![Janela Novo download](docs/screenshots/novo-download.png)

## Instalar

Os instaladores ficam na página da [última versão](https://github.com/Michaeljhoni/Fisga/releases/latest),
em **Assets**. Depois de instalar o programa, instale a [extensão do navegador](#extensão-do-navegador).

### Windows 10 e 11

1. Baixe `Fisga-<versão>-setup.exe`.
2. Abra o arquivo. O instalador não tem assinatura digital paga, então o Windows avisa na primeira
   vez: clique em **Mais informações → Executar assim mesmo**.
3. Siga o instalador. Ele não pede senha de administrador (instala só para o seu usuário), já liga o
   Fisga aos navegadores e, se você tiver o Firefox, oferece a extensão no final.

Deixe marcada a opção **Iniciar o Fisga com o Windows**: ele fica no ícone da bandeja (perto do
relógio, às vezes escondido na setinha **^**) esperando os downloads do navegador. Para remover, use
**Configurações → Aplicativos → Fisga → Desinstalar**.

### macOS 12 ou mais novo (Apple silicon: M1, M2, M3...)

1. Baixe `Fisga-<versão>.dmg` e abra.
2. Arraste o **Fisga** para a pasta **Aplicativos**.
3. Na primeira vez, clique no Fisga com o botão direito → **Abrir** → **Abrir**. O app não tem
   assinatura da Apple, que é paga; se o macOS não oferecer a opção, vá em **Ajustes do Sistema →
   Privacidade e Segurança** e clique em **Abrir Mesmo Assim**.

Ao abrir, o Fisga se liga sozinho aos navegadores. Macs com processador Intel ainda não têm versão
pronta.

### Linux

**Arch Linux e derivados** (Manjaro, EndeavourOS, CachyOS...), com os binários prontos da última
versão (pacote `fisga-bin`, instala em segundos):

```bash
git clone https://github.com/Michaeljhoni/Fisga.git
cd Fisga/packaging/arch-bin && makepkg -si
```

Para compilar o último commit em vez da versão lançada, use `packaging/arch` (pacote `fisga-git`).

**Outras distros** (Debian, Ubuntu, Fedora, openSUSE...): compile pelo script, que instala em
`~/.local`. Precisa de Rust e do GTK ≥ 4.12 com os cabeçalhos (Ubuntu 24.04 e Debian 13:
`libgtk-4-dev` e o [rustup](https://rustup.rs); Fedora: `gtk4-devel`).

```bash
git clone https://github.com/Michaeljhoni/Fisga.git
cd Fisga && scripts/install-dev.sh
```

Opcionais: `yt-dlp` e `ffmpeg` (vídeos de sites), `libnotify`, `wl-clipboard` e `clamav`.

### Extensão do navegador

- **Firefox:** [addons.mozilla.org](https://addons.mozilla.org/firefox/addon/fisga@fisga.local/)
  (em análise pela Mozilla; até sair, use o modo de desenvolvedor abaixo).
- **Chrome, Edge, Brave, Vivaldi:** a Chrome Web Store vem em breve. Por enquanto, abra
  `chrome://extensions`, ligue o **Modo do desenvolvedor**, clique em **Carregar sem compactação** e
  escolha a pasta `chrome` da extensão.
- **Firefox, modo de desenvolvedor:** `about:debugging` → **Este Firefox** → **Carregar extensão
  temporária** → o `manifest.json` da pasta `firefox` (sai quando o Firefox fecha).

A extensão vem junto com o programa:

| Sistema | Pasta da extensão |
|---|---|
| Windows | `%LOCALAPPDATA%\Programs\Fisga\share\fisga\extension` |
| macOS | `/Applications/Fisga.app/Contents/Resources/share/fisga/extension` |
| Linux (pacote) | `/usr/share/fisga/extension` |
| Linux (`install-dev.sh`) | `extension/dist`, na pasta do código |

## Pelo terminal

```bash
fisga add https://exemplo.com/arquivo.iso   # manda para a fila
fisga list                                  # o que está baixando
fisga pause 3 && fisga resume 3
fisga settings --speed-limit 2048           # no máximo 2 MB/s, somando tudo
fisga stop                                  # pausa tudo e encerra o serviço
```

No Windows, o `fisga.exe` fica em `%LOCALAPPDATA%\Programs\Fisga\bin`; no macOS, em
`/Applications/Fisga.app/Contents/MacOS`.

## Mais

- [Guia completo](docs/GUIA.md): todas as opções, a extensão, regras de janela para niri, Sway e
  Hyprland, problemas comuns e o caminho de quem vinha do LDM, o nome antigo.
- [Para quem vai mexer no código](docs/CONTINUAR.md).

## In English

**Fisga** is a free and open-source download manager for Linux, Windows and macOS, an alternative to
Internet Download Manager (IDM) written from scratch in Rust with a GTK 4 interface. It splits each
file into up to 32 connections, resumes interrupted downloads, has queues and a scheduler, and comes
with a Chrome and Firefox extension that catches the browser's downloads and adds a **Download** button
over videos on any site (YouTube and others through yt-dlp). Get the Windows installer, the macOS
`.dmg` or the Linux packages from the [latest release](https://github.com/Michaeljhoni/Fisga/releases/latest).
The interface is in Brazilian Portuguese for now.

## Licença

[GPL-3.0-or-later](LICENSE): pode usar, estudar, modificar e redistribuir; quem distribuir uma versão
modificada mantém o código aberto sob a mesma licença.
