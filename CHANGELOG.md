# Novidades

Cada versão tem uma seção `## X.Y.Z`; o texto dela vira a descrição da release no GitHub.

## 1.2.0

- **Atualização automática:** uma vez por dia o Fisga procura uma versão nova e mostra uma faixa no
  alto da janela, com as novidades. No Windows, **Atualizar agora** baixa o instalador, confere a
  soma, instala e abre o Fisga de novo; os downloads em andamento continuam depois. No macOS o
  `.dmg` é aberto; no Linux, quem atualiza é o gerenciador de pacotes. Dá para desligar em
  Preferências → Geral, e **Procurar atualizações** no menu olha na hora. Quem está na 1.1.0 instala
  esta à mão uma última vez.
- **Vídeos no Windows e no macOS sem instalar nada:** no primeiro vídeo, o Fisga baixa o yt-dlp, o
  ffmpeg e o Deno (uns 150 MB, uma vez só), conferidos pela soma, e mantém o yt-dlp em dia. Vídeos
  com acento no nome da pasta (`Vídeos`) não terminam mais com erro no Windows.
- **Link de vídeo colado:** um endereço do YouTube, Vimeo, Instagram e outros sites colado em
  **Adicionar URL** (ou no `fisga add`) baixa o vídeo, com a escolha de qualidade, e não a página.
- **Baixar todos os links:** clique direito numa página → **Baixar todos os links com o Fisga** (ou
  só os selecionados). O Fisga lista os links para arquivos, para filtrar e marcar.
- **Sites excluídos** (Preferências → Navegador): os downloads desses sites ficam com o navegador, e
  os links deles copiados não abrem o Fisga.
- **Aviso de endereço repetido** na janela *Novo download*, com onde ele já foi salvo.
- **Propriedades** de um download (clique direito ou Alt+Enter), e **arrastar e soltar** links na
  janela principal.
- **Temas:** escuro ou claro, e o tema da semana de datas comemorativas (Carnaval, Páscoa, Dia dos
  Namorados, festa junina, Halloween, Natal e Ano-novo), com uma animação atrás dos downloads que
  para quando o Fisga não está em uso. Dá para desligar em Preferências → Aparência.
- **Mais leve:** sem nada acontecendo, a janela não gasta processador.
- **macOS:** agora pede o macOS 15 ou mais novo.

## 1.1.0

O Fisga agora também roda no Windows e no macOS.

- **Windows 10 e 11:** instalador `Fisga-1.1.0-setup.exe`. Instala só para o seu usuário, sem pedir
  administrador, liga o Fisga aos navegadores (Chrome, Edge, Brave, Vivaldi, Firefox...) e oferece a
  extensão do Firefox no final. Notificações do próprio Windows com o nome do Fisga, ícone na bandeja
  e opção de iniciar com o Windows.
- **macOS 12 ou mais novo (Apple silicon):** `Fisga-1.1.0.dmg`; é só arrastar para Aplicativos. Na
  primeira vez, abra com o botão direito → **Abrir**, porque o app não tem assinatura da Apple.
  Notificações, som, ícone na barra de menus e opção de iniciar com o sistema.
- **Nos três sistemas:** suspender ou desligar ao terminar a fila, som ao concluir e captura de links
  copiados.
- **Extensão:** pacotes para a Chrome Web Store e para o addons.mozilla.org.

## 1.0.0

Primeira versão pública.

- **Downloads rápidos:** até 32 conexões por arquivo, divisão dinâmica dos trechos, conexões paradas
  assumidas por outras, e retomada de onde parou, mesmo depois de desligar o computador.
- **Extensão para Chrome e Firefox:** assume os downloads do navegador pelos tipos de arquivo da lista
  (editável em Preferências → Navegador), põe um botão **Baixar** sobre vídeos e músicas de qualquer
  site (yt-dlp para YouTube e outros) e manda um link direto para o Fisga com Alt+clique (a tecla é
  configurável).
- **Janela "Novo download":** confira nome, pasta, fila e tamanho antes de começar.
- **Filas e agendador:** quantos downloads ao mesmo tempo, horário para começar e parar, suspender ou
  desligar ao terminar.
- **Categorias** por tipo de arquivo, cada uma na sua pasta.
- **E mais:** limite de velocidade, proxy, login por site, soma SHA-256, antivírus e comando ao
  concluir, ícone na bandeja, captura de links copiados, iniciar com o sistema.
- **Pacotes para o Arch:** `fisga-bin` (binários prontos desta release) e `fisga-git` (compila o
  último commit).
