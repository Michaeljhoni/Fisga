# Guia do Fisga

Tudo o que o [README](../README.md) resume: as peças, a instalação em detalhe, cada opção da
interface e do terminal, e como resolver problemas.

> De *fisgar*: pegar o download no ar. Inspirado no comportamento do Internet Download Manager, mas
> escrito do zero, sem nada do código dele. Até outubro de 2026 se chamava LDM.

O destaque é a **extensão do navegador**: ao passar o mouse sobre um vídeo ou música em qualquer
site, aparece um botão **Baixar** no canto do player. Um clique abre a janela **Novo download** do
Fisga, como no IDM, para confirmar nome, pasta e tamanho; a setinha ▾ mostra tudo o que foi detectado
na página (stream HLS/DASH, arquivos MP4/MP3, ou "detectar automaticamente", que usa o yt-dlp em
sites como YouTube).

## Peças

| Pasta | Binário | O que faz |
|---|---|---|
| `crates/fisga-core` | — | Motor: várias conexões por arquivo, divisão dinâmica de segmentos, retomada |
| `crates/fisga-daemon` | `fisgad` | Serviço em segundo plano: lista, fila (3 simultâneos), notificações, yt-dlp |
| `crates/fisga-gui` | `fisga-gui` | Interface GTK4: janela "Novo download", lista de downloads e ícone na bandeja (só cliente do `fisgad`) |
| `crates/fisga-cli` | `fisga` | Linha de comando (`add`, `get`, `list`, `confirm`, `pause`, `resume`, `rm`, `watch`, `install-hosts`) |
| `crates/fisga-native-host` | `fisga-native-host` | Ponte entre a extensão (Native Messaging) e o `fisgad` |
| `crates/fisga-ipc` | — | Mensagens JSON pelo socket `$XDG_RUNTIME_DIR/fisga/fisgad.sock` |
| `extension/` | — | Extensão MV3 para Chrome/Chromium/Brave/Edge/Vivaldi e Firefox |

```
navegador ──Native Messaging──▶ fisga-native-host ──Unix socket──▶ fisgad ──▶ fisga-core / yt-dlp
                                              fisga (CLI) ──Unix socket──┤
                                          fisga-gui (GTK4) ──Unix socket──┘
```

## Instalar (qualquer distro)

Requisitos: Rust e GTK4 ≥ 4.12 com os cabeçalhos (Arch: `sudo pacman -S rust gtk4`; Debian 13/Ubuntu
24.04: `libgtk-4-dev` + [rustup](https://rustup.rs); Fedora: `gtk4-devel`).
Opcionais: `yt-dlp` e `ffmpeg` (vídeos de sites), `libnotify` (notificações), `wl-clipboard`
(capturar links copiados no Wayland).

```bash
scripts/install-dev.sh
```

Isso compila, instala `fisga`, `fisgad`, `fisga-native-host` e `fisga-gui` em `~/.local/bin` (e o atalho
`io.fisga.Fisga.desktop` no menu de aplicativos), registra o host nativo em todos os navegadores
encontrados e gera a extensão em `extension/dist/`. Depois carregue a extensão:

- **Chromium e derivados:** `chrome://extensions` → Modo do desenvolvedor → Carregar sem compactação →
  `extension/dist/chrome`. O ID fica fixo (`ocifdbhjaijkdgcnpigjlemkmahkojcm`) graças à chave no manifesto.
- **Firefox:** `about:debugging#/runtime/this-firefox` → Carregar extensão temporária →
  `extension/dist/firefox/manifest.json`. No popup da extensão, clique em "Permitir acesso a todos os
  sites" (o Firefox trata essa permissão como opcional). Para instalar de forma permanente, a extensão
  precisa ser assinada pela Mozilla (ou use Firefox Developer Edition/Nightly/LibreWolf com
  `xpinstall.signatures.required=false`).

Navegadores em Flatpak/Snap rodam isolados e não conseguem iniciar o host nativo sem configuração extra.

### Arch Linux: pacote (`pacman`)

`packaging/arch/PKGBUILD` gera o pacote `fisga-git`, que instala tudo em `/usr`: os programas, o
atalho no menu, o host nativo já registrado para todos os usuários (Firefox, Chromium, Chrome,
Brave, Edge, Vivaldi, LibreWolf) e a extensão em `/usr/share/fisga/extension`. Enquanto o
repositório for privado, construa a partir do seu checkout (o `makepkg` usa o que está
commitado):

```bash
cd packaging/arch
FISGA_GIT="git+file://$(realpath ../..)" makepkg -si
```

Para atualizar: `git pull` e o mesmo comando. Quem usava o `scripts/install-dev.sh` pode tirar a
instalação antiga: `fisga install-hosts --uninstall` e `rm ~/.local/bin/{fisga,fisgad,fisga-native-host,fisga-gui}
~/.local/share/applications/io.fisga.Fisga.desktop`. Na extensão, carregue `/usr/share/fisga/extension/chrome`
(ou `firefox/manifest.json`).

### Vindo do LDM (o nome antigo)

O pacote `fisga-git` substitui o `ldm-git` (o `pacman` pergunta se pode removê-lo). Antes de
atualizar, pare o antigo: `ldm stop; pkill -x ldm-gui`. Na primeira vez que o Fisga roda, ele leva
para as pastas novas o que era do LDM: `~/.config/ldm` (configurações e logins), `~/.local/share/ldm`
(a lista de downloads) e `~/.local/state/ldm` (logs). Downloads pausados continuam de onde pararam
(os arquivos `.ldm-part` ganham o nome novo), o "Iniciar com a sessão" passa a abrir o Fisga, e
comandos de "Ao concluir" que usam `$LDM_FILE` (e as outras) continuam valendo. No navegador,
recarregue a extensão: ela agora conversa com `io.fisga.native`. Instalações com o
`scripts/install-dev.sh` rodam `fisga install-hosts`, que também apaga o registro antigo
(`io.ldm.native.json`); apague à mão `~/.local/bin/{ldm,ldmd,ldm-native-host,ldm-gui}` e
`~/.local/share/applications/io.ldm.Ldm.desktop`. No Firefox, a extensão mudou de ID
(`fisga@fisga.local`): remova a antiga e carregue a nova.

## Interface gráfica (`fisga-gui`)

Visual escuro com destaque âmbar. No topo, o **gráfico de velocidade ao vivo** (o último minuto,
com o total, quantos baixando, as conexões e o tempo para terminar). Abaixo, filtros rápidos
(*Todos*, *Em andamento*, *Concluídos*, *Com erro*) e os downloads em **cartões**: um anel com o
progresso, o tamanho e o **mapa do arquivo** (quanto de cada oitavo do arquivo já chegou, com um
brilho subindo enquanto baixa; vídeos de sites enchem da esquerda para a direita, porque o yt-dlp
baixa em sequência). Os mais recentes ficam no topo; o botão de ordem no topo inverte, e o de lista
troca os cartões pela lista com colunas (as duas escolhas ficam guardadas). O botão flutuante
*Adicionar URL* fica no canto.

- **Janela principal** (`fisga-gui`, ou "Fisga" no menu de aplicativos): à esquerda, como no IDM,
  *Todos os downloads*, *Em andamento*, *Concluídos* e as **categorias** (Vídeos, Músicas, Imagens,
  Documentos, Compactados, Programas, Outros), cada uma com a contagem. À direita, a lista com nome,
  tamanho, progresso, velocidade, tempo restante e estado, atualizada em tempo real. Botões para
  adicionar URL (Ctrl+N; sugere o link da área de transferência), retomar, pausar, remover (Delete;
  pergunta se apaga também o arquivo), abrir o arquivo e abrir a pasta. No menu (☰): *Retomar
  todos*, *Pausar todos*, *Copiar endereço* (Ctrl+C), *Remover concluídos da lista*,
  *Preferências* (Ctrl+,) e *Sobre*. Clique duplo: confirma um download
  pendente, abre um concluído ou mostra o progresso dos demais. **Clique direito** num download:
  menu com *Abrir*, *Abrir pasta*, *Retomar*, *Pausar*, *Copiar endereço* e *Remover…*.
- **Busca e colunas**: a caixa de busca no topo (Ctrl+F) filtra a lista pelo nome ou pelo
  endereço (sem diferenciar maiúsculas nem acentos; várias palavras = todas precisam aparecer),
  junto com a categoria ou fila escolhida à esquerda. Em ☰ → *Colunas*, mostre ou esconda
  Tamanho, Progresso, Velocidade, Restante, Estado e *Adicionado em* (data e hora).
- **Exportar e importar a lista**: ☰ → *Exportar lista…* grava os endereços mostrados (a vista e a
  busca atuais), um por linha, num `.txt`; *Importar lista…* abre um arquivo desses na janela de
  lote, para escolher a fila e a pasta.
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
  "Capturar links copiados" ou com `fisga settings --watch-clipboard false`. Se a janela não puder
  abrir, o link copiado é descartado (não vira download sozinho).
- **Progresso** (a janela de status do IDM): abre depois de "Baixar agora", com barra, quanto já
  baixou, velocidade, tempo restante, se dá para retomar, a pasta, **Pausar/Retomar** e o **limite
  de velocidade deste download** ("No máximo ... KB/s", que vale na hora e se soma ao limite geral:
  o menor vence; também pelo clique direito → *Limitar velocidade…*). Ao terminar
  vira **Download concluído**, com **Abrir** e **Abrir pasta**. Pode ser desligada nas Preferências.
- **Preferências** (aba *Geral*): pasta de downloads, subpastas por tipo de arquivo,
  conexões por arquivo, a janela de progresso, capturar links copiados e o **limite de velocidade** (KB/s para todos os
  downloads juntos; muda na hora, até nos que estão baixando, e aparece na barra de status). Ficam
  no daemon, em `~/.config/fisga/settings.json`. Vídeos de sites (yt-dlp) recebem o limite por
  processo (`--limit-rate`). Também ali: **ícone na bandeja** e **iniciar com a sessão** (veja abaixo).
- **Bandeja**: com a opção ligada (padrão), o Fisga mostra um ícone na bandeja do sistema
  (StatusNotifierItem: waybar, noctalia, KDE, GNOME com a extensão AppIndicator). Fechar a janela
  principal deixa o Fisga ali, pronto para confirmar os downloads do navegador. Clique no ícone para
  abrir a janela; o menu tem o resumo (quantos baixando, velocidade, fila), *Adicionar URL…*,
  *Retomar todos*, *Pausar todos* e *Sair* (só fecha a interface; os downloads continuam no `fisgad`).
  Sem bandeja disponível, fechar a janela fecha o Fisga, como antes. No waybar, ative o módulo `tray`.
- **Iniciar com a sessão**: cria `~/.config/autostart/io.fisga.Fisga.desktop`, que roda
  `fisga-gui --background` ao entrar: sem janela, só o ícone (que aparece quando a bandeja subir). O
  GNOME e o KDE leem essa pasta; no niri, Sway e Hyprland ela vale quando a sessão é iniciada pelo
  systemd (o `niri-session`, por exemplo). Se não funcionar, adicione ao `~/.config/niri/config.kdl`:
  `spawn-at-startup "fisga-gui" "--background"`.
- **Filas** (aba *Filas* das Preferências, ou ☰ → *Filas e agendador*), como no IDM: além da *Fila
  principal*, crie outras ("Noite", "Trabalho"...), cada uma com seu número de downloads ao mesmo
  tempo e seu agendador. Elas aparecem na barra lateral. A janela *Novo download* tem o campo
  *Fila* (quando há mais de uma). No clique direito de um download: *Mover para cima*, *Mover para
  baixo* e *Mover para a fila ▸*. Excluir uma fila manda os downloads dela para a principal.
- **Adicionar em lote** (o *Batch download* do IDM; ☰ → *Adicionar em lote…* ou Ctrl+Shift+N):
  um endereço com `*` e um intervalo (`aula_*.mp4` de `01` a `20`, ou `parte-*.rar` de `a` a
  `f`), ou uma lista de links colada (se a área de transferência tiver vários links, a lista já
  vem preenchida). Mostra quantos endereços e o primeiro e o último antes de adicionar. Vão
  direto para a fila escolhida (ou pausados), sem a janela *Novo download* para cada um.
- **Baixar de um site** (o *Site Grabber* do IDM; ☰ → *Baixar de um site…* ou Ctrl+Shift+G): dê o
  endereço de uma página e quantos níveis seguir (0 = só ela). O Fisga lê a página e as páginas
  abaixo dela no mesmo site (não sobe para a pasta de cima) e lista os arquivos que elas apontam.
  Filtre por tipo (Vídeos, Documentos...) e por nome (`*.pdf aula_??.mp4`), marque o que quiser e
  mande para uma fila. Usa o proxy e os logins por site. Lê até 100 páginas por busca.
- **Atualizar endereço** (o *Refresh download address* do IDM): no clique direito de um download
  cujo link expirou (comum em sites de arquivos e links assinados), *Atualizar endereço…*. Cole um
  novo link do mesmo arquivo, ou clique em *Capturar do navegador*: a página abre, você clica no
  link de novo, e o Fisga usa esse link (com os cookies novos) em vez de criar outro download. Nos
  dois casos o download continua de onde parou. O Fisga confere o tamanho e recusa um link de outro
  arquivo. A captura funciona com links diretos de tamanho conhecido e com a extensão pedindo
  confirmação (o padrão).
- **Rede** (aba *Rede* das Preferências):
  - **Proxy**: *Do sistema* (as variáveis `http_proxy`/`https_proxy`, o padrão), *Sem proxy* ou
    *Manual*, com `http://host:porta` ou `socks5://usuário:senha@host:porta`. Vale para todos os
    downloads, inclusive os vídeos de sites (o yt-dlp recebe o proxy pelo ambiente, para a senha
    não aparecer na lista de processos).
  - **Logins por site** (os *Site logins* do IDM): usuário e senha enviados aos links diretos de
    um site (autenticação HTTP), só por https. `*.exemplo.com` vale também para os subdomínios.
    Se o site redireciona para outro endereço (um CDN), a senha e os cookies não vão junto. Ficam
    em `~/.config/fisga/logins.json`, legível só pelo seu usuário, nunca no log.
  - Trocar o proxy vale para os downloads que começarem depois; os que estão baixando continuam
    pelo caminho antigo até serem pausados e retomados.
- **Conferir a soma** (SHA-256, ou SHA-1/384/512): cole a soma publicada na página de download
  no campo *Soma* da janela *Novo download* (ou `fisga add --checksum`), e o Fisga lê o arquivo ao
  terminar e confere. Se não bater, avisa, marca o download ("Concluído — atenção: o SHA-256 não
  confere") e não roda as ações abaixo. Depois, no menu da linha, *Conferir soma (SHA-256)…*
  mostra a SHA-256 do arquivo ou confere com outra soma; aceita o texto como as páginas
  mostram: só os dígitos, `sha256:…`, ou uma linha de um arquivo `SHA256SUMS`.
- **Ao concluir** (aba *Ao concluir* das Preferências), nesta ordem:
  - **Verificar com o antivírus**: o ClamAV (`clamdscan`, ou `clamscan` sem o serviço). Se achar
    um vírus, avisa, marca o download ("Concluído — atenção: ...") e não roda o comando.
  - **Rodar um comando**, no shell, com o arquivo em `$FISGA_FILE` (e `$FISGA_NAME`, `$FISGA_URL`,
    `$FISGA_DIR`), por exemplo `mv "$FISGA_FILE" ~/Concluídos/`. O nome do arquivo nunca entra no
    texto do comando, só nas variáveis: use-as entre aspas.
  - **Tocar um som** (ligado por padrão): o "concluído" do tema de sons, com `canberra-gtk-play`,
    `paplay` ou `pw-play`.
- **Navegador** (aba *Navegador* das Preferências), como *Tipos de arquivo* e *Teclas* do IDM:
  - **Tipos de arquivo**: as extensões dos downloads do navegador que vão para o Fisga, separadas
    por espaço (a lista padrão já traz vídeos, músicas, compactados, programas e modelos de IA:
    `safetensors`, `ckpt`, `gguf`, `pt`, `pth`, `onnx`). Vídeos, músicas e compactados também são
    reconhecidos pelo tipo que o site informa. O nome que só chega no `Content-Disposition` (como
    nos links do Civitai, `/api/download/models/…`) também conta.
  - **Tecla para baixar com o Fisga** (padrão: Alt): segurada ao clicar num link, manda o link ao
    Fisga, seja qual for o tipo; o navegador não abre nem baixa. Também Ctrl+Alt, Shift+Alt,
    Ctrl+Shift ou nenhuma.
  - A extensão pede essas opções ao app pelo `fisga-native-host` (ao iniciar, ao trocar de aba e a
    cada download, no máximo a cada 5 s) e guarda as últimas; com o `fisgad` parado, valem as
    guardadas. No terminal: `fisga settings --file-types "zip safetensors gguf"` (`padrao` volta à
    lista padrão) e `fisga settings --force-key ctrl+alt`.
- **Agendador** (por fila, como o *Scheduler* do IDM): a fila só roda das HH:MM às HH:MM nos
  dias escolhidos. Fora desse horário os downloads ficam *na fila*; *Baixar agora* e retomar um
  download à mão começam na hora mesmo assim. No horário de parada, os downloads da fila voltam
  para ela e continuam na próxima vez. Sem horário de parada, a fila vai até acabar. **Ao terminar
  a fila**: não fazer nada, suspender ou desligar o computador (`systemctl suspend`/`poweroff`), com
  uma janela que conta 60 segundos e deixa cancelar (ou `fisga schedule --cancel`). Isso só acontece
  quando a fila começou no horário (ou o agendador foi configurado dentro dele) e pelo menos um
  download terminou; ligar o computador no meio do horário nunca o desliga sozinho.

### Categorias e pastas

Cada download recebe uma categoria pela extensão do arquivo (vídeos de sites via yt-dlp são
*Vídeos*, a não ser que o arquivo seja só áudio). Como no IDM, por padrão cada categoria vai para
uma subpasta da pasta de downloads: `~/Downloads/Vídeos`, `Músicas`, `Imagens`, `Documentos`,
`Compactados` (zip, rar, 7z, tar.*) e `Programas` (AppImage, deb, rpm, flatpak, iso, exe...);
o que não se encaixa fica em `~/Downloads`. A pasta escolhida na janela "Novo download" ou com
`-o` no terminal sempre vale. Para tudo na mesma pasta: desligue "Separar por tipo de arquivo" nas
Preferências, ou `fisga settings --category-folders false`.

A GUI é só um cliente do `fisgad`: fechar a janela não interrompe os downloads.

### Do clique no navegador à janela de confirmação

1. A extensão manda `{"type":"download", …, "confirm": true}` ao `fisga-native-host` (o popup tem a
   opção **"Confirmar cada download numa janela do Fisga"**; desmarcada, envia `"confirm": false` e o
   download começa direto, como antes). Mensagens sem o campo também pedem confirmação.
2. O host inicia o `fisgad` se preciso e envia `Request::Add` com `confirm: true`; responde à extensão
   `{"type":"added","id":…,"pending":true}`, e o botão mostra "Confirme no Fisga".
3. O `fisgad` cria o download no estado **pendente** (`State::Pending`, "aguardando" no `fisga list`) e
   descobre tamanho e nome: links diretos com uma requisição `Range`, vídeos de sites com
   `yt-dlp -J`. Se o yt-dlp disser que o endereço é uma lista (feed, busca, canal, playlist), o
   download fica marcado com erro e não pode ser confirmado. A mudança vai para todos os assinantes
   (`Request::Subscribe`).
4. Se há uma GUI conectada (assinante com `confirmer: true`), ela abre a janela **Novo download**.
   Senão, o `fisgad` executa `fisga-gui --pending`, que mostra só essa janela e fecha depois da resposta.
5. A janela responde com `Request::Confirm { id, now, file_name, output_dir, format, conflict }`
   (Baixar agora / Adicionar à fila; `format` é a qualidade escolhida, `conflict` o que fazer com um
   arquivo que já existe: `rename`, `overwrite` ou `resume`) ou `Request::Remove` (Cancelar).
6. Se o `fisga-gui` não existir, ou não se conectar em 15 s (sem display, por exemplo), o download vai
   para a fila sozinho, com uma notificação: um clique no navegador nunca se perde.

O `fisgad` herda o ambiente de quem o iniciou (normalmente o navegador, com `WAYLAND_DISPLAY`), e é
esse ambiente que o `fisga-gui` recebe. `FISGA_GUI=/caminho/fisga-gui` troca o executável; `FISGA_GUI=`
(vazio) desliga a janela e todo download do navegador vai direto para a fila.

## Uso pela linha de comando

```bash
fisga add https://exemplo.com/arquivo.iso --wait   # envia ao daemon e acompanha
fisga get https://exemplo.com/arquivo.iso -c 16    # baixa direto, sem daemon (Ctrl+C pausa)
fisga add https://exemplo.com/arquivo.iso --confirm # abre a janela "Novo download"
fisga confirm 4 --queue -o ~/ISOs                   # responde a um download pendente pelo terminal
fisga confirm 5 --quality 720                      # vídeo de site em 720p (ou --quality audio)
fisga confirm 6 --if-exists overwrite              # arquivo já existe: rename, overwrite ou resume
fisga list
fisga pause 3 && fisga resume 3
fisga rm 3 --delete-file
fisga watch                                        # mudanças em tempo real
fisga settings                                     # configurações (pasta, subpastas, fila, conexões)
fisga settings --category-folders false --max-parallel 5
fisga settings --speed-limit 2048                  # 2 MB/s para tudo (0 = sem limite)
fisga limit 7 512                                  # só o download 7: no máximo 512 KB/s (0 = tira)
fisga batch 'https://site/aula_*.mp4' --from 01 --to 20   # em lote: aula_01.mp4 ... aula_20.mp4
fisga batch --file links.txt -Q 1 --paused         # uma lista de links ("-" = entrada padrão)
fisga batch 'https://site/parte-*.rar' --from a --to f --dry-run   # só mostra os endereços
fisga export --unfinished > pendentes.txt          # os endereços da lista (importe com fisga batch --file)
fisga grab https://site/pub/ --depth 1 --type documento         # lista os PDFs, DOCs... do site
fisga grab https://site/aulas/ --pattern 'aula_*.mp4' --add -Q 1  # e manda para a fila 1
fisga add https://site/distro.iso --checksum 3a7b…   # confere a SHA-256 da página ao terminar
fisga verify 7                                     # a SHA-256 do arquivo do download 7
fisga verify 7 "$(grep distro.iso SHA256SUMS)"     # confere com a linha do SHA256SUMS
fisga settings --antivirus true --sound false      # ao concluir: ClamAV sim, som não
fisga settings --on-complete 'mv "$FISGA_FILE" ~/Concluídos/'   # comando ao concluir ("" = nenhum)
fisga settings --watch-clipboard false             # não abrir a janela para links copiados
fisga settings --tray false                        # sem ícone na bandeja
fisga schedule --on --start 02:00 --stop 08:00     # a fila só roda de madrugada
fisga schedule --days seg,ter,qua,qui,sex --when-done desligar
fisga schedule --stop nenhum                       # vai até terminar a fila
fisga schedule --cancel                            # cancela o desligamento que está contando
fisga schedule --off
fisga queue add Noite --max-parallel 1             # outra fila (fisga queue = lista)
fisga schedule -Q 1 --on --start 23:00 --stop 06:00 # agendador da fila 1
fisga add https://exemplo.com/arquivo.iso -Q 1     # direto na fila 1
fisga move 7 -Q 1                                  # move o download 7 para a fila 1
fisga move 7 --top                                 # primeiro da fila (--up, --down, --bottom)
fisga queue rm 1                                   # exclui (os downloads vão para a principal)
fisga refresh 7 https://exemplo.com/novo-link      # link expirado: novo endereço do mesmo arquivo
fisga refresh 7 --capture                          # usa o próximo link do mesmo arquivo vindo do navegador
fisga settings --proxy socks5://127.0.0.1:1080     # ou http://host:3128, "nenhum", "sistema"
fisga settings --proxy -                           # lê o endereço (com senha) da entrada padrão
fisga login add arquivos.exemplo.com ana           # pede a senha (lida da entrada padrão)
fisga login                                        # lista os sites com login (sem as senhas)
fisga login rm arquivos.exemplo.com
```

## Desenvolvimento

```bash
cargo test --workspace          # testes (incluem um servidor HTTP local com Range)
cargo build --workspace --bins --examples && scripts/e2e.sh   # ponta a ponta: daemon, CLI, host nativo e confirmação (servidor local)
node extension/tools/make-icons.mjs
node scripts/dev-server.mjs     # abra http://localhost:8787/extension/test/overlay.html
```

No Windows/macOS dá para compilar e testar dentro de um container Linux: `scripts/docker-cargo.sh test --workspace`.
O script monta, na primeira vez, a imagem `fisga-build` (`scripts/Dockerfile.build`: imagem oficial do
Rust em Debian trixie + `libgtk-4-dev`).

### Comportamento no niri / Wayland

O botão flutuante é desenhado dentro da página pelo navegador, então funciona igual em qualquer
compositor (niri, Hyprland, Sway, GNOME, KDE) e no X11.

A GUI usa GTK4 com `app-id` fixo `io.fisga.Fisga`. A janela principal se chama sempre `Fisga`; as outras
(*Novo download*, a de progresso, *Download concluído*, *Preferências*) são diálogos. Num compositor
*tiling* eles abririam como colunas inteiras; para abrirem flutuantes, como no IDM, adicione ao
`~/.config/niri/config.kdl`:

```kdl
window-rule {
    match app-id=r#"^io.fisga.Fisga$"#
    exclude title="^Fisga$"
    open-floating true
    open-focused true
}
```

(Quando a janela principal está aberta, os diálogos já nascem ligados a ela e o niri os deixa
flutuantes sozinho; a regra vale para quando só o diálogo aparece, ao clicar no navegador.)
Equivalentes: Sway, `for_window [app_id="io.fisga.Fisga" title="^(?!Fisga$)"] floating enable`;
Hyprland, `windowrulev2 = float, class:^(io.fisga.Fisga)$, title:negative:^Fisga$`.

### Problemas

- **Parar tudo:** `fisga stop` pausa todos os downloads e encerra o daemon (`fisga list` / `fisga pause N`
  para um só).
- **Logs:** `~/.local/state/fisga/fisgad.log` (daemon) e `~/.local/state/fisga/fisga-gui.log` (interface,
  quando aberta pelo menu ou pelo daemon).
- **"Fisga" no menu não abre nada:** rode `fisga-gui` num terminal para ver o erro; reinstale com
  `scripts/install-dev.sh`, que grava o atalho com o caminho completo do executável.
- **Vídeo falha com "HTTP Error 410: Gone" (ou 403):** o link do vídeo que a extensão capturou (um
  `master.m3u8`, por exemplo) era assinado e expirou. O Fisga já tenta de novo pela página de onde ele
  veio; se também falhar, recarregue a página, dê play e baixe logo em seguida.

## Estado

- [x] Motor segmentado com retomada, testado: uma conexão TCP por segmento, conexão parada assumida
      por outra, menos conexões quando o servidor pede (503/429), progresso salvo só depois dos dados
- [x] Daemon, CLI, host nativo
- [x] Extensão: botão sobre vídeo/áudio, menu de mídias detectadas, captura de downloads, menu de contexto
- [x] Interface gráfica (GTK4): janela "Novo download" ao clicar no botão, lista com ações
- [x] Janela de progresso e de download concluído, categorias por tipo de arquivo (barra lateral e
      subpastas), Preferências
- [x] Limite de velocidade global (muda na hora)
- [x] Ícone na bandeja, iniciar com a sessão, agendador (suspender/desligar ao terminar)
- [x] Várias filas, cada uma com limite e agendador, e ordem dentro da fila
- [ ] HLS/DASH nativo (hoje via yt-dlp), FTP
- [x] Pacote para o Arch (`packaging/arch/PKGBUILD`, pronto para o AUR quando o repositório for público)
- [ ] Flatpak, .deb/.rpm
