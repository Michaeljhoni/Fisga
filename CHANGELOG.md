# Novidades

Cada versão tem uma seção `## X.Y.Z`; o texto dela vira a descrição da release no GitHub.

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
