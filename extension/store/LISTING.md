# Textos para as lojas

O que colar em cada campo da Chrome Web Store e do addons.mozilla.org (AMO). Os pacotes saem de
`bash extension/build.sh` (em `extension/dist-store/`) ou do artefato `extension-stores` da CI; as
imagens são as desta pasta (`node extension/tools/make-store-images.mjs` as refaz).

## Comum às duas lojas

**Nome:** Integração Fisga (vem do manifesto: "Fisga Integration" em inglês)

**Resumo** (até 132 caracteres):
> Baixe vídeos, músicas e arquivos com o Fisga: downloads acelerados, com pausa e retomada, direto do navegador.

**Resumo em inglês** (vem do manifesto):
> Download videos, music and files with Fisga, the free download manager.

**Descrição** (texto puro: as lojas não leem Markdown). Em português:

```text
A Integração Fisga liga o seu navegador ao Fisga, um gerenciador de downloads livre e gratuito (licença GPL-3.0) para Windows, macOS e Linux, no estilo do Internet Download Manager.

O Fisga acelera os downloads abrindo várias conexões para o mesmo arquivo. Ele também permite pausar e continuar depois, mesmo após fechar o computador, organizar os downloads em filas e agendar horários. Esta extensão faz a ponte: tudo o que você quer baixar no navegador vai direto para o Fisga.

O QUE A EXTENSÃO FAZ

• Botão "Baixar" sobre vídeos e músicas: ao passar o mouse sobre um player, em qualquer site, aparece um botão no canto. Um clique manda a mídia para o Fisga; o × esconde o botão naquele vídeo.
• Escolha do que baixar: a setinha ao lado do botão lista tudo o que foi detectado na página, como transmissões HLS/DASH e arquivos MP4 e MP3, além da opção de detectar automaticamente a melhor qualidade em sites de vídeo.
• Captura dos downloads do navegador: ao clicar num link de arquivo (programas, compactados, vídeos, documentos…), o download vai para o Fisga em vez do baixador do navegador. Os tipos de arquivo e os sites que ficam de fora são escolhidos nas preferências do Fisga, e a captura pode ser desligada no popup.
• Menu do botão direito: "Baixar link com o Fisga", "Baixar esta mídia com o Fisga", "Baixar todos os links com o Fisga" (você escolhe quais na lista) e "Baixar os links selecionados".
• Tecla de atalho: segure Alt (ou a combinação escolhida no Fisga) ao clicar num link para mandá-lo ao Fisga, seja ele o que for.
• Downloads que exigem login: a extensão repassa ao aplicativo os cookies do site do arquivo, só para aquele download, para que funcionem arquivos de áreas restritas.
• Confirmação antes de baixar: cada download abre a janela "Novo download" do Fisga, onde você confere o nome, a pasta e o tamanho. Pelo popup dá para baixar direto, sem confirmar.
• Popup da extensão: mostra se o Fisga está conectado, liga e desliga cada função e lista as mídias detectadas na aba atual.

COMO USAR

1. Instale o aplicativo Fisga no computador: https://github.com/Michaeljhoni/Fisga/releases
2. Instale esta extensão.
3. Pronto: passe o mouse sobre um vídeo ou clique num link de download.

A extensão precisa do aplicativo Fisga instalado; sozinha ela não baixa nada. Sem o aplicativo, o popup avisa que ele não foi encontrado.

PRIVACIDADE

A extensão conversa apenas com o aplicativo Fisga instalado no seu computador. Nada é enviado a servidores do Fisga (eles não existem), não há rastreamento nem anúncios, e nenhum dado é vendido ou compartilhado. Política de privacidade: https://github.com/Michaeljhoni/Fisga/blob/main/PRIVACY.md

CÓDIGO ABERTO

O Fisga e esta extensão têm código aberto, que qualquer um pode ler e conferir: https://github.com/Michaeljhoni/Fisga
Dúvidas, sugestões e problemas: https://github.com/Michaeljhoni/Fisga/issues
```

Em inglês (o idioma padrão da página, porque o manifesto tem `default_locale: en`):

```text
Fisga Integration connects your browser to Fisga, a free and open source download manager (GPL-3.0) for Windows, macOS and Linux, in the style of Internet Download Manager.

Fisga speeds up downloads by opening several connections to the same file. It can also pause and resume them later, even after the computer is turned off, organize downloads in queues and schedule them. This extension is the bridge: whatever you want to download in the browser goes straight to Fisga.

WHAT THE EXTENSION DOES

• "Download" button over videos and music: hover a player on any site and a button appears in its corner. One click sends the media to Fisga; the × hides the button on that video.
• Choose what to download: the arrow next to the button lists everything detected on the page, such as HLS/DASH streams and MP4 and MP3 files, plus automatic detection of the best quality on video sites.
• Takes over browser downloads: when you click a link to a file (programs, archives, videos, documents…), the download goes to Fisga instead of the browser's downloader. The file types and the excluded sites are chosen in Fisga's preferences, and the takeover can be turned off in the popup.
• Right-click menu: "Download link with Fisga", "Download this media with Fisga", "Download all links with Fisga" (you pick which ones from the list) and "Download selected links".
• Shortcut key: hold Alt (or the combination chosen in Fisga) while clicking a link to send it to Fisga, whatever it is.
• Downloads that need a login: the extension passes the cookies of the file's site to the app, only for that download, so files behind a login work too.
• Confirmation before downloading: each download opens Fisga's "New download" window, where you check the name, folder and size. The popup can turn this off to download right away.
• Extension popup: shows whether Fisga is connected, turns each feature on or off and lists the media detected in the current tab.

HOW TO USE

1. Install the Fisga app on your computer: https://github.com/Michaeljhoni/Fisga/releases
2. Install this extension.
3. Done: hover a video or click a download link.

The extension needs the Fisga app installed; on its own it downloads nothing. Without the app, the popup says it was not found.

PRIVACY

The extension talks only to the Fisga app installed on your computer. Nothing is sent to Fisga servers (there are none), there is no tracking and no ads, and no data is sold or shared. Privacy policy: https://github.com/Michaeljhoni/Fisga/blob/main/PRIVACY.md

OPEN SOURCE

Fisga and this extension are open source, so anyone can read and check the code: https://github.com/Michaeljhoni/Fisga
Questions, suggestions and issues: https://github.com/Michaeljhoni/Fisga/issues
```

**Política de privacidade (URL):** https://github.com/Michaeljhoni/Fisga/blob/main/PRIVACY.md

**Site / página de suporte:** https://github.com/Michaeljhoni/Fisga — issues em
https://github.com/Michaeljhoni/Fisga/issues

## Chrome Web Store

- **ID na loja:** `jjpcgkejmdnabpjfdfeagdnobpimlhnb` (está nos `allowed_origins` do native host
  desde a 1.3.0; enviar à análise um pacote só depois de publicada uma versão do Fisga com ele).
- **O pacote não baixa do YouTube** (a loja recusou a 1.3.0 por isso): o `build.sh` tira o content
  script do YouTube e recusa os endereços dele. Não citar o YouTube nos textos da loja, salvo o aviso
  abaixo.
- **Aviso na descrição** (só na Chrome Web Store, depois da lista "O que a extensão faz"):
  > Por regra da Chrome Web Store, esta versão não baixa do YouTube.

  Em inglês:
  > Due to Chrome Web Store policy, this version does not download from YouTube.
- **Categoria:** Ferramentas (Tools). **Idiomas:** inglês (o padrão) e português (Brasil), cada um
  com a sua descrição.
- **Imagens:** ícone 128×128 (`extension/src/icons/icon-128.png`), bloco promocional pequeno
  `promo-440x280.png`, capturas `screenshot-1-1280x800.png` e `screenshot-2-1280x800.png`.
- **Instruções de teste** (Acesso → Instruções de teste):

```text
This extension is the browser side of Fisga, a free download manager: https://github.com/Michaeljhoni/Fisga. It talks only to the Fisga app installed on the same computer, through native messaging (host "io.fisga.native"), and sends nothing to any server.
To test: install Fisga 1.3.1 or later from https://github.com/Michaeljhoni/Fisga/releases (Windows installer, macOS .dmg, or Linux packages; on Linux run "fisga install-hosts"). Then open a page with a video, for example https://www.w3schools.com/html/html5_video.asp, hover the player and click "Download", or right-click a link and choose "Download link with Fisga". The "New download" window of the app opens. Without the app, the popup says the app was not found.
This version does not work on YouTube, per the Chrome Web Store policy: the content script is excluded from YouTube pages, and the extension refuses YouTube addresses.
```

**Aba "Práticas de privacidade":**

- **Finalidade única:**
  > Mandar para o gerenciador de downloads Fisga, instalado no computador, os arquivos e mídias que
  > o usuário quer baixar.
- **Justificativas das permissões:**
  - `nativeMessaging`: é como a extensão fala com o aplicativo Fisga instalado no computador, que
    faz os downloads.
  - `downloads`: reconhecer os downloads iniciados no navegador e passá-los ao Fisga, quando o
    usuário deixa essa opção ligada.
  - `webRequest`: reconhecer pelas respostas dos sites os arquivos de mídia (vídeos e áudios) que a
    página carrega, para oferecer o botão "Baixar".
  - `cookies`: downloads que exigem login precisam dos cookies do site do arquivo; a extensão os
    repassa só ao aplicativo local, só para aquele download.
  - `tabs`: saber a página e a aba de onde veio cada mídia detectada.
  - `contextMenus`: os itens "Baixar link/esta mídia com o Fisga" do menu do botão direito.
  - `storage`: guardar as preferências da extensão.
  - **Permissão de host (todos os sites):** o botão "Baixar" precisa aparecer nos players e links
    de qualquer site que o usuário visite.
- **Código remoto:** Não, não uso código remoto.
- **Uso de dados:** não marcar nenhuma categoria (nada sai do computador do usuário; os dados vão
  só ao aplicativo local) e marcar as três declarações (não vende, não usa para outros fins, não
  usa para crédito).

## addons.mozilla.org (AMO)

- **Distribuição:** "Neste site" (listada), para ter a página da loja.
- **Categoria:** Gerenciamento de downloads (Download Management). **Licença:** GNU GPL v3.0 ou
  posterior.
- **Imagens:** as duas capturas 1280×800.
- **Código-fonte:** responder "Não": o JavaScript vai como está, sem minificar nem empacotar.
- **Notas para o revisor** (em inglês):
  > This extension is the browser side of Fisga, a free (GPL-3.0) download manager:
  > https://github.com/Michaeljhoni/Fisga. It talks only to the Fisga application installed on the
  > same computer, through native messaging (host "io.fisga.native"); it sends nothing to any server.
  > To test it with the app: install a release from https://github.com/Michaeljhoni/Fisga/releases
  > (on Linux, "fisga install-hosts" registers the native host), then click "Download" over a video
  > or right-click a link and pick "Download link with Fisga". Without the app, the popup says that
  > the app was not found.
