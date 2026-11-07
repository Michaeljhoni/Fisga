# Política de privacidade — Fisga

*English below.*

O Fisga (o aplicativo e a extensão "Integração Fisga" para Chrome e Firefox) **não coleta, não
guarda em servidores e não envia a ninguém** dados de quem o usa. Não há contas, anúncios,
estatísticas de uso nem servidores do Fisga.

## O que a extensão faz com os dados

Quando você pede um download (o botão "Baixar", o menu de contexto ou um download do navegador
que o Fisga assume), a extensão passa ao aplicativo Fisga **instalado no seu próprio computador**,
pelo canal de "native messaging" do navegador, só o necessário para baixar aquele arquivo:

- o endereço do arquivo e da página onde ele está;
- os cookies e cabeçalhos (como Referer e User-Agent) que o site exige para liberar o arquivo,
  como o próprio navegador faria.

Esses dados não saem do seu computador por causa do Fisga: o aplicativo os usa apenas para pedir
o arquivo ao mesmo site, e os cookies de vídeos ficam num arquivo temporário apagado ao fim do
download.

As preferências da extensão (mostrar o botão, capturar downloads, pedir confirmação) ficam no
armazenamento local do navegador.

## Programas de vídeo

No Windows e no macOS, ao pedir o primeiro vídeo de um site, o aplicativo baixa o yt-dlp, o ffmpeg e
o Deno das páginas de cada projeto no GitHub, e procura uma versão nova do yt-dlp no máximo uma vez
por dia em que você baixar vídeos. Essas conexões não levam nenhum dado seu além do que qualquer
download leva (o endereço IP, por exemplo).

## Versões novas

Uma vez por dia, o aplicativo pergunta à API do GitHub qual é a última versão publicada do Fisga
(`api.github.com/repos/Michaeljhoni/Fisga/releases/latest`). O pedido leva só o que qualquer pedido
leva: o endereço IP e a versão do Fisga, no User-Agent. Ao atualizar, o instalador vem da página da
versão no GitHub. Isso pode ser desligado em Preferências → Geral → "Procurar versões novas do Fisga".

## Permissões

- **nativeMessaging**: falar com o aplicativo Fisga instalado no computador.
- **downloads**, **webRequest**: reconhecer downloads e arquivos de mídia para oferecer o Fisga.
- **cookies**: repassar ao aplicativo os cookies do site do arquivo, para downloads que exigem login.
- **tabs**, **contextMenus**, **storage**: o botão nas páginas, o item "Baixar com o Fisga" do menu
  e as preferências.
- **acesso a todos os sites**: o botão "Baixar" aparece nos players e links de qualquer site.

## Contato

Dúvidas: abra uma issue em <https://github.com/Michaeljhoni/Fisga/issues>.

---

# Privacy policy — Fisga

Fisga (the application and the "Fisga Integration" extension for Chrome and Firefox) **does not
collect, store on servers or share** any user data. There are no accounts, ads, usage statistics
or Fisga servers.

When you ask for a download, the extension hands the Fisga application **installed on your own
computer**, through the browser's native messaging, only what that download needs: the file's and
the page's address, and the cookies and headers the site requires to serve the file. Fisga does
not send this data off your computer; the application only uses it to request the file from the
same site, and cookies for videos are kept in a temporary file deleted when the download ends.
The extension's preferences stay in the browser's local storage.

On Windows and macOS, the first video you ask for makes the application fetch yt-dlp, ffmpeg and Deno
from each project's GitHub pages, and it looks for a new yt-dlp at most once a day when you download
videos. These connections carry nothing about you beyond what any download does (your IP address,
for example).

Once a day the application asks GitHub's API which is Fisga's latest published version
(`api.github.com/repos/Michaeljhoni/Fisga/releases/latest`). The request carries only what any request
does: your IP address and Fisga's version, in the User-Agent. When updating, the installer comes from
the version's GitHub page. This can be turned off in Preferências → Geral → "Procurar versões novas do
Fisga".

Questions: open an issue at <https://github.com/Michaeljhoni/Fisga/issues>.
