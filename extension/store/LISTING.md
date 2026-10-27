# Textos para as lojas

O que colar em cada campo da Chrome Web Store e do addons.mozilla.org (AMO). Os pacotes saem de
`bash extension/build.sh` (em `extension/dist-store/`) ou do artefato `extension-stores` da CI; as
imagens são as desta pasta (`node extension/tools/make-store-images.mjs` as refaz).

## Comum às duas lojas

**Nome:** Integração Fisga (vem do manifesto: "Fisga Integration" em inglês)

**Resumo** (até 132 caracteres):
> Baixe vídeos, músicas e arquivos com o Fisga: downloads acelerados, com pausa e retomada, direto do navegador.

**Descrição:**
> A Integração Fisga liga o navegador ao Fisga, um gerenciador de downloads livre (GPL-3.0) que
> acelera downloads abrindo várias conexões por arquivo, com pausa, retomada, filas e agendamento.
>
> • Um botão "Baixar" aparece sobre vídeos e players: um clique manda a mídia para o Fisga.
> • Downloads do navegador passam para o Fisga sozinhos (dá para desligar no popup).
> • "Baixar link com o Fisga" e "Baixar esta mídia com o Fisga" no menu do botão direito.
> • Downloads que exigem login funcionam: a extensão repassa ao app os cookies do site do arquivo.
>
> Precisa do aplicativo Fisga instalado no computador: https://github.com/Michaeljhoni/Fisga
> Nada é enviado a servidores do Fisga (não existem); veja a política de privacidade.

**Política de privacidade (URL):** https://github.com/Michaeljhoni/Fisga/blob/main/PRIVACY.md

**Site / página de suporte:** https://github.com/Michaeljhoni/Fisga — issues em
https://github.com/Michaeljhoni/Fisga/issues

## Chrome Web Store

- **Categoria:** Ferramentas (Tools). **Idioma:** Português (Brasil).
- **Imagens:** ícone 128×128 (`extension/src/icons/icon-128.png`), bloco promocional pequeno
  `promo-440x280.png`, capturas `screenshot-1-1280x800.png` e `screenshot-2-1280x800.png`.

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
