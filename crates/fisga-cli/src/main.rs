mod hosts;

use std::path::PathBuf;
use std::time::Duration;

use anyhow::{bail, Context};
use clap::{Parser, Subcommand};
use indicatif::{ProgressBar, ProgressStyle};
use fisga_core::{DownloadOptions, EngineEvent, Outcome, RequestSpec, Target};
use fisga_ipc::{
    AddRequest, BrowserSettings, Category, Checksum, Conflict, Connection, Direction, DownloadInfo, ForceKey, Hashing,
    ProxyMode, ProxySettings, Queue, Request, Response, Settings, SiteLogin, State, TimeOfDay, WhenDone, MAIN_QUEUE,
    WEEKDAYS,
};
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

#[derive(Parser)]
#[command(name = "fisga", version, about = "Fisga: gerenciador de downloads para Linux")]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(clap::Args)]
struct Source {
    url: String,
    /// Pasta de destino (padrão: pasta de Downloads)
    #[arg(short = 'o', long)]
    dir: Option<PathBuf>,
    /// Nome do arquivo (padrão: detectado pelo servidor)
    #[arg(short, long)]
    name: Option<String>,
    /// Número máximo de conexões
    #[arg(short, long)]
    connections: Option<usize>,
    #[arg(long)]
    referer: Option<String>,
    #[arg(long)]
    user_agent: Option<String>,
    /// Cabeçalho extra, no formato "Nome: valor" (pode repetir)
    #[arg(short = 'H', long = "header")]
    headers: Vec<String>,
}

impl Source {
    fn headers(&self) -> anyhow::Result<Vec<(String, String)>> {
        self.headers
            .iter()
            .map(|h| {
                let (name, value) = h.split_once(':').with_context(|| format!("cabeçalho inválido: {h}"))?;
                Ok((name.trim().to_string(), value.trim().to_string()))
            })
            .collect()
    }
}

#[derive(clap::Args)]
struct SettingsArgs {
    /// Pasta de downloads ("padrao" volta para a pasta de Downloads do sistema)
    #[arg(long)]
    dir: Option<PathBuf>,
    /// Separar por tipo de arquivo em subpastas (Vídeos, Músicas, Programas...)
    #[arg(long)]
    category_folders: Option<bool>,
    /// Downloads ao mesmo tempo
    #[arg(long)]
    max_parallel: Option<usize>,
    /// Conexões por arquivo
    #[arg(long)]
    connections: Option<usize>,
    /// Abrir a janela de progresso ao confirmar um download
    #[arg(long)]
    show_progress: Option<bool>,
    /// Limite de velocidade de todos os downloads juntos, em KB/s (0 = sem limite)
    #[arg(long)]
    speed_limit: Option<u64>,
    /// Abrir a janela "Novo download" ao copiar o link de um arquivo
    #[arg(long)]
    watch_clipboard: Option<bool>,
    /// Ícone na bandeja (o fisga-gui continua rodando ao fechar a janela)
    #[arg(long)]
    tray: Option<bool>,
    /// Proxy: "sistema" (variáveis http_proxy...), "nenhum", um endereço como
    /// http://host:3128, ou "-" para ler o endereço da entrada padrão (com senha,
    /// socks5://usuário:senha@host:1080, sem deixá-la no histórico do shell)
    #[arg(long)]
    proxy: Option<String>,
    /// Tocar um som ao concluir um download
    #[arg(long)]
    sound: Option<bool>,
    /// Verificar cada download concluído com o ClamAV
    #[arg(long)]
    antivirus: Option<bool>,
    /// Comando a rodar ao concluir (no shell, com o arquivo em "$FISGA_FILE"; "" = nenhum)
    #[arg(long)]
    on_complete: Option<String>,
    /// Extensões que o navegador entrega ao Fisga, separadas por espaço ("padrao" = a lista padrão)
    #[arg(long)]
    file_types: Option<String>,
    /// Tecla que, segurada ao clicar num link, manda o link ao Fisga: nenhuma, alt, ctrl+alt, shift+alt ou ctrl+shift
    #[arg(long, value_parser = parse_force_key)]
    force_key: Option<ForceKey>,
}

#[derive(Subcommand)]
enum LoginCommand {
    /// Lista os sites com login (sem as senhas)
    List,
    /// Guarda o usuário e a senha de um site; a senha é lida da entrada padrão
    Add { host: String, user: String },
    /// Esquece o login de um site
    Rm { host: String },
}

#[derive(clap::Args)]
struct ScheduleArgs {
    /// Liga o agendador: a fila só roda no horário
    #[arg(long, conflicts_with = "off")]
    on: bool,
    /// Desliga o agendador: a fila roda sempre
    #[arg(long)]
    off: bool,
    /// Horário de início, HH:MM
    #[arg(long, value_parser = parse_time)]
    start: Option<TimeOfDay>,
    /// Horário de parada, HH:MM, ou "nenhum" (vai até terminar a fila)
    #[arg(long, value_parser = parse_stop)]
    stop: Option<Stop>,
    /// Dias: "todos" ou uma lista como "seg,ter,qua,qui,sex"
    #[arg(long, value_parser = |text: &str| parse_days(text).map(Days))]
    days: Option<Days>,
    /// Ao terminar a fila: nada, suspender ou desligar
    #[arg(long, value_parser = parse_when_done)]
    when_done: Option<WhenDone>,
    /// Cancela a suspensão ou o desligamento planejado
    #[arg(long, conflicts_with_all = ["on", "off", "start", "stop", "days", "when_done"])]
    cancel: bool,
    /// Fila (número em "fisga queue"; padrão: a principal)
    #[arg(short = 'Q', long, default_value_t = MAIN_QUEUE)]
    queue_id: u64,
}

#[derive(Subcommand)]
enum QueueCommand {
    /// Lista as filas
    List,
    /// Cria uma fila
    Add {
        name: String,
        /// Downloads desta fila ao mesmo tempo
        #[arg(long, default_value_t = 1)]
        max_parallel: usize,
    },
    /// Muda o nome ou o número de downloads ao mesmo tempo de uma fila
    Set {
        id: u64,
        #[arg(long)]
        name: Option<String>,
        #[arg(long)]
        max_parallel: Option<usize>,
    },
    /// Exclui uma fila (os downloads dela vão para a fila principal)
    Rm { id: u64 },
}

#[derive(Subcommand)]
enum Command {
    /// Envia um download para o daemon (fisgad)
    Add {
        #[command(flatten)]
        source: Source,
        /// Acompanha o progresso até terminar
        #[arg(short, long)]
        wait: bool,
        /// Pede confirmação na janela "Novo download" (fisga-gui) antes de começar
        #[arg(long, conflicts_with = "wait")]
        confirm: bool,
        /// Fila do download (número em "fisga queue")
        #[arg(short = 'Q', long)]
        queue_id: Option<u64>,
        /// Soma SHA-256 (ou SHA-1/384/512) publicada na página: o arquivo é conferido ao terminar
        #[arg(long, value_parser = Checksum::parse)]
        checksum: Option<Checksum>,
    },
    /// Confere a soma SHA-256 (ou SHA-1/384/512) do arquivo de um download
    Verify {
        id: u64,
        /// A soma publicada na página do download; sem ela, só mostra a SHA-256 do arquivo
        #[arg(value_parser = Checksum::parse)]
        checksum: Option<Checksum>,
    },
    /// Limite de velocidade de um download, em KB/s (0 = só o limite geral)
    Limit { id: u64, kb_per_second: u64 },
    /// Escreve os endereços da lista, um por linha (importe com "fisga batch --file")
    Export {
        /// Só os que ainda não terminaram
        #[arg(long)]
        unfinished: bool,
        /// Só os desta fila (número em "fisga queue")
        #[arg(short = 'Q', long)]
        queue_id: Option<u64>,
    },
    /// Procura os arquivos de um site (o "Grabber" do IDM): lista, ou adiciona com --add
    Grab {
        url: String,
        /// Quantos níveis de páginas seguir (0 = só esta página; máximo 3)
        #[arg(short, long, default_value_t = 1)]
        depth: u8,
        /// Só estes tipos: video, musica, imagem, documento, compactado, programa, outro
        #[arg(short = 't', long = "type", value_parser = |text: &str| parse_categories(text).map(Categories))]
        types: Option<Categories>,
        /// Só nomes assim, com * e ? (ex.: "*.pdf aula_??.mp4")
        #[arg(short, long, default_value = "")]
        pattern: String,
        /// Adiciona os encontrados à fila em vez de só listar
        #[arg(long)]
        add: bool,
        /// Pasta de destino (com --add)
        #[arg(short = 'o', long, requires = "add")]
        dir: Option<PathBuf>,
        /// Fila (com --add)
        #[arg(short = 'Q', long, requires = "add")]
        queue_id: Option<u64>,
        /// Adiciona pausados (com --add)
        #[arg(long, requires = "add")]
        paused: bool,
    },
    /// Vários downloads de uma vez (o "download em lote" do IDM), direto na fila
    Batch {
        /// Endereço com um * no lugar do número ou da letra, ex.: https://site/aula_*.mp4
        #[arg(required_unless_present = "file", requires_all = ["from", "to"])]
        template: Option<String>,
        /// Primeiro número ou letra (01 deixa os números com 2 dígitos)
        #[arg(long)]
        from: Option<String>,
        /// Último número ou letra
        #[arg(long)]
        to: Option<String>,
        /// Dígitos dos números, ex.: 3 dá 001, 002... (padrão: os do início)
        #[arg(long, default_value_t = 0)]
        width: usize,
        /// Arquivo com os endereços, um por linha ("-" = entrada padrão)
        #[arg(long, conflicts_with = "template")]
        file: Option<PathBuf>,
        /// Pasta de destino (padrão: a de cada tipo de arquivo)
        #[arg(short = 'o', long)]
        dir: Option<PathBuf>,
        /// Fila (número em "fisga queue")
        #[arg(short = 'Q', long)]
        queue_id: Option<u64>,
        /// Adiciona pausados, sem entrar na fila
        #[arg(long)]
        paused: bool,
        /// Só mostra os endereços, sem adicionar
        #[arg(long)]
        dry_run: bool,
    },
    /// Atualiza o endereço de um download cujo link expirou (continua de onde parou)
    Refresh {
        id: u64,
        /// O novo endereço do mesmo arquivo
        #[arg(required_unless_present_any = ["capture", "cancel"], conflicts_with_all = ["capture", "cancel"])]
        url: Option<String>,
        /// Espera o navegador mandar o mesmo arquivo de novo e usa o link dele
        #[arg(long, conflicts_with = "cancel")]
        capture: bool,
        /// Para de esperar o novo endereço
        #[arg(long)]
        cancel: bool,
    },
    /// Usuário e senha por site (autenticação HTTP dos links diretos)
    Login {
        #[command(subcommand)]
        command: Option<LoginCommand>,
    },
    /// Mostra, cria, altera ou exclui filas
    Queue {
        #[command(subcommand)]
        command: Option<QueueCommand>,
    },
    /// Move um download para outra fila, ou dentro da fila dele
    Move {
        id: u64,
        /// Para o fim desta fila
        #[arg(short = 'Q', long, conflicts_with_all = ["up", "down", "top", "bottom"])]
        queue_id: Option<u64>,
        #[arg(long)]
        up: bool,
        #[arg(long)]
        down: bool,
        /// Para o começo da fila
        #[arg(long)]
        top: bool,
        /// Para o fim da fila
        #[arg(long)]
        bottom: bool,
    },
    /// Baixa em primeiro plano, sem o daemon
    Get {
        #[command(flatten)]
        source: Source,
    },
    /// Lista os downloads
    List,
    /// Confirma um download que aguarda na janela "Novo download"
    Confirm {
        id: u64,
        /// Coloca na fila em vez de começar agora
        #[arg(long)]
        queue: bool,
        /// Pasta de destino
        #[arg(short = 'o', long)]
        dir: Option<PathBuf>,
        /// Nome do arquivo
        #[arg(short, long)]
        name: Option<String>,
        /// Qualidade de um vídeo de site: altura (1080, 720...) ou "audio"
        #[arg(short, long)]
        quality: Option<String>,
        /// Se o arquivo já existir: rename (novo nome), overwrite (substituir) ou resume (continuar)
        #[arg(long, value_parser = parse_conflict)]
        if_exists: Option<Conflict>,
        /// Fila do download (número em "fisga queue")
        #[arg(short = 'Q', long)]
        queue_id: Option<u64>,
        /// Soma que o arquivo deve ter (SHA-256, SHA-1, SHA-384 ou SHA-512)
        #[arg(long, value_parser = Checksum::parse)]
        checksum: Option<Checksum>,
    },
    /// Pausa um download
    Pause { id: u64 },
    /// Retoma um download pausado ou com erro
    Resume { id: u64 },
    /// Remove um download da lista
    Rm {
        id: u64,
        /// Apaga também o arquivo (ou as partes baixadas)
        #[arg(long)]
        delete_file: bool,
    },
    /// Mostra as mudanças em tempo real
    Watch,
    /// Registra o host nativo nos navegadores (Chrome, Chromium, Brave, Edge, Vivaldi, Firefox...)
    InstallHosts {
        /// ID extra de extensão Chrome autorizada (ex.: ao carregar sem a chave do manifesto)
        #[arg(long = "chrome-id")]
        chrome_ids: Vec<String>,
        /// Remove os registros em vez de criar
        #[arg(long, conflicts_with = "system")]
        uninstall: bool,
        /// Para pacotes: grava os registros do sistema (todos os usuários) dentro desta pasta ($pkgdir)
        #[arg(long, requires = "host_path")]
        system: Option<PathBuf>,
        /// Para pacotes: onde o fisga-native-host fica depois de instalado (ex.: /usr/bin/fisga-native-host)
        #[arg(long, requires = "system")]
        host_path: Option<PathBuf>,
    },
    /// Mostra ou altera as configurações do daemon
    Settings(SettingsArgs),
    /// Mostra ou altera o agendador (horário da fila e o que fazer ao terminar)
    Schedule(ScheduleArgs),
    /// Encerra o daemon (pausa os downloads em andamento)
    Stop,
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    match Cli::parse().command {
        Command::Add { source, wait, confirm, queue_id, checksum } => add(source, wait, confirm, queue_id, checksum).await,
        Command::Verify { id, checksum } => verify(id, checksum).await,
        Command::Confirm { id, queue, dir, name, quality, if_exists, queue_id, checksum } => {
            let output_dir = dir.map(|d| std::path::absolute(&d).unwrap_or(d));
            let request = Request::Confirm {
                id,
                now: !queue,
                file_name: name,
                output_dir,
                format: quality,
                conflict: if_exists,
                queue: queue_id,
                checksum,
            };
            simple(request).await
        }
        Command::Grab { url, depth, types, pattern, add, dir, queue_id, paused } => {
            fisga_ipc::ensure_daemon().await?;
            eprintln!("procurando em {url} (profundidade {depth})…");
            let Response::Grabbed { links, pages } = fisga_ipc::call(&Request::Grab { url, depth }).await? else {
                bail!("resposta inesperada do daemon");
            };
            let types = types.map(|Categories(types)| types).unwrap_or_default();
            let found: Vec<_> = links.iter().filter(|link| fisga_ipc::grab_filter(link, &types, &pattern)).collect();
            eprintln!("{} arquivo(s) em {pages} página(s); {} com os filtros", links.len(), found.len());
            if !add {
                found.iter().for_each(|link| println!("{}", link.url));
                return Ok(());
            }
            if found.is_empty() {
                bail!("nada para adicionar");
            }
            let urls = found.iter().map(|link| link.url.clone()).collect();
            let output_dir = dir.map(|d| std::path::absolute(&d).unwrap_or(d));
            let Response::AddedBatch { ids } =
                fisga_ipc::call(&Request::AddBatch { urls, output_dir, queue: queue_id, start: !paused }).await?
            else {
                bail!("resposta inesperada do daemon");
            };
            println!("{} downloads adicionados", ids.len());
            Ok(())
        }
        Command::Batch { template, from, to, width, file, dir, queue_id, paused, dry_run } => {
            let urls = match (template, file) {
                (Some(template), _) => {
                    let (from, to) = (from.unwrap_or_default(), to.unwrap_or_default());
                    fisga_ipc::expand_batch(&template, &from, &to, width).map_err(anyhow::Error::msg)?
                }
                (None, Some(file)) => {
                    let text = match file.as_os_str() == "-" {
                        true => std::io::read_to_string(std::io::stdin())?,
                        false => std::fs::read_to_string(&file).with_context(|| format!("não li {}", file.display()))?,
                    };
                    fisga_ipc::parse_url_list(&text)
                }
                (None, None) => bail!("diga o endereço com * (e --from/--to) ou --file"),
            };
            if urls.is_empty() {
                bail!("nenhum endereço encontrado");
            }
            if dry_run {
                urls.iter().for_each(|url| println!("{url}"));
                return Ok(());
            }
            fisga_ipc::ensure_daemon().await?;
            let output_dir = dir.map(|d| std::path::absolute(&d).unwrap_or(d));
            let request = Request::AddBatch { urls, output_dir, queue: queue_id, start: !paused };
            let Response::AddedBatch { ids } = fisga_ipc::call(&request).await? else {
                bail!("resposta inesperada do daemon");
            };
            match (ids.first(), ids.last()) {
                (Some(first), Some(last)) if ids.len() > 1 => {
                    println!("{} downloads adicionados (#{first} a #{last})", ids.len())
                }
                _ => println!("1 download adicionado"),
            }
            Ok(())
        }
        Command::Export { unfinished, queue_id } => {
            let Response::Downloads { mut items } = fisga_ipc::call(&Request::List).await? else {
                bail!("resposta inesperada do daemon");
            };
            items.sort_by_key(|item| (item.order, item.id));
            items
                .iter()
                .filter(|item| !unfinished || item.state != State::Completed)
                .filter(|item| queue_id.map_or(true, |queue| item.queue == queue))
                .for_each(|item| println!("{}", item.url));
            Ok(())
        }
        Command::Limit { id, kb_per_second } => {
            simple(Request::SetSpeedLimit { id, limit: kb_per_second * 1024 }).await?;
            match kb_per_second {
                0 => println!("download #{id}: sem limite próprio"),
                kb => println!("download #{id}: no máximo {}/s", bytes(kb * 1024)),
            }
            Ok(())
        }
        Command::Refresh { id, url, capture, cancel } => match url {
            Some(url) => {
                simple(Request::RefreshUrl { id, url }).await?;
                println!("endereço atualizado: o download #{id} continua de onde parou");
                Ok(())
            }
            None => {
                simple(Request::AwaitUrl { id, waiting: capture && !cancel }).await?;
                if capture {
                    println!("aguardando: abra a página do arquivo e clique no link de novo no navegador");
                }
                Ok(())
            }
        },
        Command::Queue { command } => queues(command.unwrap_or(QueueCommand::List)).await,
        Command::Login { command } => logins(command.unwrap_or(LoginCommand::List)).await,
        Command::Move { id, queue_id, up, down, top, bottom } => {
            let direction = [(up, Direction::Up), (down, Direction::Down), (top, Direction::Top), (bottom, Direction::Bottom)]
                .into_iter()
                .find_map(|(chosen, direction)| chosen.then_some(direction));
            match (queue_id, direction) {
                (Some(queue), _) => simple(Request::SetQueue { id, queue }).await,
                (None, Some(direction)) => simple(Request::Reorder { id, direction }).await,
                (None, None) => bail!("diga para onde: --queue-id N, --up, --down, --top ou --bottom"),
            }
        }
        Command::Get { source } => get(source).await,
        Command::List => list().await,
        Command::Pause { id } => simple(Request::Pause { id }).await,
        Command::Resume { id } => simple(Request::Resume { id }).await,
        Command::Rm { id, delete_file } => simple(Request::Remove { id, delete_file }).await,
        Command::Watch => watch().await,
        Command::InstallHosts { chrome_ids, uninstall, system, host_path } => match (system, host_path) {
            (Some(root), Some(host)) => hosts::install_system(&root, &host, &chrome_ids),
            _ => hosts::install(&chrome_ids, uninstall),
        },
        Command::Settings(args) => settings(args).await,
        Command::Schedule(args) => schedule(args).await,
        Command::Stop => match fisga_ipc::call(&Request::Shutdown).await {
            Ok(_) => Ok(()),
            Err(_) => {
                eprintln!("o daemon não está rodando");
                Ok(())
            }
        },
    }
}

fn bytes(n: u64) -> String {
    indicatif::HumanBytes(n).to_string()
}

fn bar_style() -> ProgressStyle {
    ProgressStyle::with_template("{spinner} [{bar:32}] {bytes}/{total_bytes} {binary_bytes_per_sec} ETA {eta} {msg}")
        .expect("valid template")
        .progress_chars("=> ")
}

async fn get(source: Source) -> anyhow::Result<()> {
    let client = fisga_core::build_client()?;
    let mut headers = source.headers()?;
    if let Some(r) = &source.referer {
        headers.push(("Referer".into(), r.clone()));
    }
    if let Some(ua) = &source.user_agent {
        headers.push(("User-Agent".into(), ua.clone()));
    }
    let spec = RequestSpec { url: source.url.clone(), headers };
    let dir = source.dir.clone().unwrap_or_else(|| std::env::current_dir().unwrap_or_default());
    let opts = DownloadOptions { connections: source.connections.unwrap_or(8), ..Default::default() };
    let cancel = CancellationToken::new();
    let ctrl_c = cancel.clone();
    tokio::spawn(async move {
        if tokio::signal::ctrl_c().await.is_ok() {
            ctrl_c.cancel();
        }
    });

    let (tx, mut events) = mpsc::unbounded_channel();
    let bar = ProgressBar::new_spinner();
    bar.enable_steady_tick(Duration::from_millis(120));
    let target = Target::Dir { dir, file_name: source.name.clone() };
    let download = fisga_core::download(&client, &spec, target, &opts, tx, cancel);
    tokio::pin!(download);
    let outcome = loop {
        tokio::select! {
            Some(event) = events.recv() => match event {
                EngineEvent::Started { path, total, resumable } => {
                    if let Some(total) = total {
                        bar.set_length(total);
                        bar.set_style(bar_style());
                    }
                    bar.println(format!(
                        "→ {} {}",
                        path.display(),
                        if resumable { "(retomável)" } else { "(sem retomada: o servidor não aceita Range)" }
                    ));
                }
                EngineEvent::Progress(p) => {
                    bar.set_position(p.downloaded);
                    bar.set_message(format!("{} conexões", p.active_connections));
                }
            },
            outcome = &mut download => break outcome,
        }
    };
    bar.finish_and_clear();
    match outcome {
        Ok(Outcome::Completed(path)) => println!("✓ {}", path.display()),
        Ok(Outcome::Paused(path)) => println!("⏸ pausado: {} (rode o mesmo comando para retomar)", path.display()),
        Err(fisga_core::Error::Cancelled) => println!("cancelado"),
        Err(e) => bail!(e),
    }
    Ok(())
}

async fn add(
    source: Source,
    wait: bool,
    confirm: bool,
    queue: Option<u64>,
    checksum: Option<Checksum>,
) -> anyhow::Result<()> {
    fisga_ipc::ensure_daemon().await?;
    let request = AddRequest {
        queue,
        url: source.url.clone(),
        output_dir: source.dir.clone().map(|d| std::path::absolute(&d).unwrap_or(d)),
        file_name: source.name.clone(),
        connections: source.connections,
        referrer: source.referer.clone(),
        user_agent: source.user_agent.clone(),
        headers: source.headers()?,
        start: true,
        confirm,
        checksum,
        ..Default::default()
    };
    let mut connection = Connection::open().await?;
    let Response::Added { id } = connection.call(&Request::Add(request)).await? else {
        bail!("resposta inesperada do daemon");
    };
    if confirm {
        println!("download #{id} aguardando confirmação");
    } else {
        println!("download #{id} adicionado");
    }
    if !wait {
        return Ok(());
    }

    let mut connection = Connection::open().await?;
    connection.send(&Request::Subscribe { confirmer: false }).await?;
    let bar = ProgressBar::new_spinner();
    bar.enable_steady_tick(Duration::from_millis(120));
    while let Some(message) = connection.recv().await? {
        let item = match message {
            Response::Update { item } if item.id == id => item,
            Response::Downloads { items } => match items.into_iter().find(|i| i.id == id) {
                Some(item) => item,
                None => continue,
            },
            Response::Removed { id: removed } if removed == id => bail!("o download foi removido"),
            _ => continue,
        };
        if let Some(total) = item.total {
            if bar.length() != Some(total) {
                bar.set_length(total);
                bar.set_style(bar_style());
            }
        }
        bar.set_position(item.downloaded);
        bar.set_message(item.name.clone());
        let checked = item.checksum.is_none() || matches!(item.hash, Some(Hashing::Done { .. } | Hashing::Failed { .. }));
        match item.state {
            // With a checksum, done once the file is checked.
            State::Completed if !checked => bar.set_message(format!("conferindo a soma de {}", item.name)),
            State::Completed => {
                bar.finish_and_clear();
                if let Some(problem) = check_problem(&item) {
                    bail!(problem);
                }
                println!("✓ {}", item.path.map(|p| p.display().to_string()).unwrap_or(item.name));
                return Ok(());
            }
            State::Failed => {
                bar.finish_and_clear();
                bail!(item.error.unwrap_or_else(|| "falhou".into()));
            }
            State::Paused => {
                bar.finish_and_clear();
                println!("⏸ pausado");
                return Ok(());
            }
            _ => {}
        }
    }
    Ok(())
}

fn parse_force_key(value: &str) -> Result<ForceKey, String> {
    let value = value.to_ascii_lowercase().replace([' ', '-', '_'], "+");
    ForceKey::ALL
        .into_iter()
        .find(|key| key.label().to_ascii_lowercase() == value)
        .ok_or_else(|| "use nenhuma, alt, ctrl+alt, shift+alt ou ctrl+shift".into())
}

fn parse_conflict(value: &str) -> Result<Conflict, String> {
    match value {
        "rename" => Ok(Conflict::Rename),
        "overwrite" => Ok(Conflict::Overwrite),
        "resume" => Ok(Conflict::Resume),
        _ => Err("use rename, overwrite ou resume".into()),
    }
}

async fn settings(args: SettingsArgs) -> anyhow::Result<()> {
    let SettingsArgs {
        dir,
        category_folders,
        max_parallel,
        connections,
        show_progress,
        speed_limit,
        watch_clipboard,
        tray,
        proxy,
        sound,
        antivirus,
        on_complete,
        file_types,
        force_key,
    } = args;
    let mut current = get_settings().await?;
    if let Some(proxy) = &proxy {
        let proxy = match proxy.trim() {
            "-" => read_secret("Endereço do proxy (com usuário:senha@, se precisar)")?,
            other => other.to_string(),
        };
        current.proxy = match proxy.trim() {
            "sistema" | "system" => ProxySettings { mode: ProxyMode::System, url: current.proxy.url.clone() },
            "nenhum" | "none" | "" => ProxySettings { mode: ProxyMode::None, url: current.proxy.url.clone() },
            url => ProxySettings { mode: ProxyMode::Manual, url: url.to_string() },
        };
    }
    let changed = dir.is_some()
        || category_folders.is_some()
        || max_parallel.is_some()
        || connections.is_some()
        || show_progress.is_some()
        || speed_limit.is_some()
        || watch_clipboard.is_some()
        || tray.is_some()
        || proxy.is_some()
        || sound.is_some()
        || antivirus.is_some()
        || on_complete.is_some()
        || file_types.is_some()
        || force_key.is_some();
    if let Some(types) = file_types {
        current.browser.file_types = match types.trim() {
            "padrao" | "padrão" | "default" => BrowserSettings::default().file_types,
            types => BrowserSettings::parse_file_types(types),
        };
    }
    current.browser.force_key = force_key.unwrap_or(current.browser.force_key);
    current.on_complete.sound = sound.unwrap_or(current.on_complete.sound);
    current.on_complete.antivirus = antivirus.unwrap_or(current.on_complete.antivirus);
    if let Some(command) = on_complete {
        current.on_complete.command = command.trim().to_string();
    }
    if let Some(dir) = dir {
        current.download_dir = if dir.as_os_str() == "padrao" { None } else { Some(std::path::absolute(&dir)?) };
    }
    current.category_folders = category_folders.unwrap_or(current.category_folders);
    current.max_parallel = max_parallel.unwrap_or(current.max_parallel);
    current.connections = connections.unwrap_or(current.connections);
    current.show_progress = show_progress.unwrap_or(current.show_progress);
    current.speed_limit = speed_limit.map(|kb| kb * 1024).unwrap_or(current.speed_limit);
    current.watch_clipboard = watch_clipboard.unwrap_or(current.watch_clipboard);
    current.tray = tray.unwrap_or(current.tray);
    if changed {
        fisga_ipc::call(&Request::SetSettings { settings: current.clone() }).await?;
    }
    let dir = current.download_dir.as_ref().map(|d| d.display().to_string()).unwrap_or_else(|| "(Downloads do sistema)".into());
    println!("pasta de downloads:      {dir}");
    println!("subpastas por tipo:      {}", if current.category_folders { "sim" } else { "não" });
    println!("downloads ao mesmo tempo: {}", current.max_parallel);
    println!("conexões por arquivo:    {}", current.connections);
    println!("janela de progresso:     {}", if current.show_progress { "sim" } else { "não" });
    match current.speed_limit {
        0 => println!("limite de velocidade:    sem limite"),
        limit => println!("limite de velocidade:    {}/s", bytes(limit)),
    }
    println!("capturar links copiados: {}", if current.watch_clipboard { "sim" } else { "não" });
    println!("ícone na bandeja:        {}", if current.tray { "sim" } else { "não" });
    println!("agendador:               {}", schedule_summary(&current));
    println!("proxy:                   {}", current.proxy.describe());
    let done = &current.on_complete;
    println!("ao concluir, som:        {}", if done.sound { "sim" } else { "não" });
    println!("ao concluir, antivírus:  {}", if done.antivirus { "sim (ClamAV)" } else { "não" });
    match done.command.as_str() {
        "" => println!("ao concluir, comando:    nenhum"),
        command => println!("ao concluir, comando:    {command}"),
    }
    println!("tipos do navegador:      {}", current.browser.file_types.join(" "));
    println!("tecla do navegador:      {}", current.browser.force_key.label());
    Ok(())
}

/// One line from the standard input; at a terminal, asked without echoing it.
fn read_secret(prompt: &str) -> anyhow::Result<String> {
    use std::io::{BufRead, IsTerminal};
    let terminal = std::io::stdin().is_terminal();
    if terminal {
        eprint!("{prompt}: ");
        set_echo(false);
    }
    let mut line = String::new();
    let read = std::io::stdin().lock().read_line(&mut line);
    if terminal {
        set_echo(true);
        eprintln!();
    }
    read?;
    Ok(line.trim_end_matches(['\r', '\n']).to_string())
}

/// Whether the terminal shows what is typed.
#[cfg(unix)]
fn set_echo(on: bool) {
    let arg = if on { "echo" } else { "-echo" };
    let _ = std::process::Command::new("stty").arg(arg).stdin(std::process::Stdio::inherit()).status();
}

#[cfg(windows)]
fn set_echo(on: bool) {
    use std::ffi::c_void;
    const STD_INPUT_HANDLE: u32 = -10i32 as u32;
    const ENABLE_ECHO_INPUT: u32 = 0x0004;
    #[link(name = "kernel32")]
    extern "system" {
        fn GetStdHandle(which: u32) -> *mut c_void;
        fn GetConsoleMode(console: *mut c_void, mode: *mut u32) -> i32;
        fn SetConsoleMode(console: *mut c_void, mode: u32) -> i32;
    }
    // SAFETY: the handle is the process's own console input; failures are ignored.
    unsafe {
        let console = GetStdHandle(STD_INPUT_HANDLE);
        let mut mode = 0;
        if GetConsoleMode(console, &mut mode) != 0 {
            SetConsoleMode(console, if on { mode | ENABLE_ECHO_INPUT } else { mode & !ENABLE_ECHO_INPUT });
        }
    }
}

async fn logins(command: LoginCommand) -> anyhow::Result<()> {
    fisga_ipc::ensure_daemon().await?;
    let Response::Logins { mut logins } = fisga_ipc::call(&Request::GetLogins).await? else {
        bail!("resposta inesperada do daemon");
    };
    match command {
        LoginCommand::List => {}
        LoginCommand::Add { host, user } => {
            let password = read_secret(&format!("Senha de {user} em {host}"))?;
            logins.push(SiteLogin { host, user, password });
        }
        LoginCommand::Rm { host } => {
            let host = SiteLogin::normalize_host(&host);
            let before = logins.len();
            logins.retain(|login| login.host != host);
            if logins.len() == before {
                bail!("nenhum login para {host}");
            }
        }
    }
    fisga_ipc::call(&Request::SetLogins { logins }).await?;
    let Response::Logins { logins } = fisga_ipc::call(&Request::GetLogins).await? else {
        bail!("resposta inesperada do daemon");
    };
    if logins.is_empty() {
        println!("nenhum login por site");
    }
    for login in logins {
        println!("{:<30} {}", login.host, login.user);
    }
    Ok(())
}

async fn get_settings() -> anyhow::Result<Settings> {
    fisga_ipc::ensure_daemon().await?;
    match fisga_ipc::call(&Request::GetSettings).await? {
        Response::Settings { settings } => Ok(settings),
        _ => bail!("resposta inesperada do daemon"),
    }
}

fn schedule_text(queue: &Queue) -> String {
    match queue.schedule.enabled {
        true => queue.schedule.describe(),
        false => "desligado (a fila roda sempre)".into(),
    }
}

fn schedule_summary(settings: &Settings) -> String {
    schedule_text(&settings.all_queues()[0])
}

async fn schedule(args: ScheduleArgs) -> anyhow::Result<()> {
    if args.cancel {
        fisga_ipc::ensure_daemon().await?;
        fisga_ipc::call(&Request::CancelPower).await?;
        println!("cancelado");
        return Ok(());
    }
    let mut settings = get_settings().await?;
    let Some(mut queue) = settings.queue(args.queue_id) else { bail!("fila não encontrada: {}", args.queue_id) };
    let before = queue.clone();
    let schedule = &mut queue.schedule;
    if args.on || args.off {
        schedule.enabled = args.on;
    }
    schedule.start = args.start.unwrap_or(schedule.start);
    schedule.stop = args.stop.map_or(schedule.stop, |Stop(stop)| stop);
    if let Some(Days(days)) = args.days {
        schedule.days = days;
    }
    schedule.when_done = args.when_done.unwrap_or(schedule.when_done);
    if queue != before {
        settings.set_queue(queue.clone());
        fisga_ipc::call(&Request::SetSettings { settings }).await?;
    }
    println!("fila:             {}", queue.name);
    println!("agendador:        {}", schedule_text(&queue));
    println!("ao terminar:      {}", queue.schedule.when_done.label().to_lowercase());
    Ok(())
}

async fn queues(command: QueueCommand) -> anyhow::Result<()> {
    let mut settings = get_settings().await?;
    match command {
        QueueCommand::List => {}
        QueueCommand::Add { name, max_parallel } => {
            let id = settings.new_queue_id();
            settings.set_queue(Queue { id, name, max_parallel, ..Queue::default() });
        }
        QueueCommand::Set { id, name, max_parallel } => {
            let Some(mut queue) = settings.queue(id) else { bail!("fila não encontrada: {id}") };
            if let Some(name) = name {
                if id == MAIN_QUEUE {
                    bail!("a fila principal não pode ser renomeada");
                }
                queue.name = name;
            }
            queue.max_parallel = max_parallel.unwrap_or(queue.max_parallel);
            settings.set_queue(queue);
        }
        QueueCommand::Rm { id } => {
            if id == MAIN_QUEUE {
                bail!("a fila principal não pode ser excluída");
            }
            if settings.queue(id).is_none() {
                bail!("fila não encontrada: {id}");
            }
            settings.remove_queue(id);
        }
    }
    fisga_ipc::call(&Request::SetSettings { settings: settings.clone() }).await?;
    // Read back: the daemon names and limits them (sanitized).
    for queue in get_settings().await?.all_queues() {
        let at_once = match queue.max_parallel {
            1 => "1 por vez".to_string(),
            n => format!("{n} ao mesmo tempo"),
        };
        println!("{:>4}  {:<20} {:<18} agendador: {}", queue.id, queue.name, at_once, schedule_text(&queue));
    }
    Ok(())
}

fn parse_time(value: &str) -> Result<TimeOfDay, String> {
    TimeOfDay::parse(value).ok_or_else(|| "use HH:MM, como 02:00 ou 23:30".into())
}

/// `--days seg,qua,sex`: one argument holding a list (clap reads an
/// `Option<Vec<T>>` field as a repeated argument instead).
#[derive(Debug, Clone)]
struct Days(Vec<u8>);

/// `--type video,documento`, for the same reason.
#[derive(Debug, Clone)]
struct Categories(Vec<Category>);

/// `--stop`: a time, or none (the queue runs until it is empty).
#[derive(Debug, Clone, PartialEq)]
struct Stop(Option<TimeOfDay>);

fn parse_stop(value: &str) -> Result<Stop, String> {
    match value {
        "nenhum" | "none" | "" => Ok(Stop(None)),
        value => parse_time(value).map(|time| Stop(Some(time))),
    }
}

fn parse_days(value: &str) -> Result<Vec<u8>, String> {
    if matches!(value, "todos" | "all") {
        return Ok((0..7).collect());
    }
    let mut days = value
        .split(',')
        .map(|day| {
            let day = day.trim().to_lowercase().replace("sab", "sáb");
            WEEKDAYS.iter().position(|w| *w == day).map(|i| i as u8).ok_or_else(|| format!("dia desconhecido: {day}"))
        })
        .collect::<Result<Vec<u8>, String>>()?;
    days.sort_unstable();
    days.dedup();
    Ok(days)
}

/// "video,documento" → [Video, Document] (with or without accents, singular or plural).
fn parse_categories(value: &str) -> Result<Vec<Category>, String> {
    value
        .split(',')
        .map(|word| {
            let word = word.trim().to_lowercase();
            let word = word.trim_end_matches('s');
            match word {
                "video" | "vídeo" => Ok(Category::Video),
                "musica" | "música" | "audio" | "áudio" => Ok(Category::Music),
                "imagem" | "image" | "imagen" | "foto" => Ok(Category::Image),
                "documento" | "doc" => Ok(Category::Document),
                "compactado" | "arquivo" => Ok(Category::Compressed),
                "programa" => Ok(Category::Program),
                "outro" => Ok(Category::Other),
                other => Err(format!("tipo desconhecido: {other} (use video, musica, imagem, documento, compactado, programa ou outro)")),
            }
        })
        .collect()
}

fn parse_when_done(value: &str) -> Result<WhenDone, String> {
    match value {
        "nada" | "nothing" => Ok(WhenDone::Nothing),
        "suspender" | "suspend" => Ok(WhenDone::Suspend),
        "desligar" | "poweroff" => Ok(WhenDone::Poweroff),
        _ => Err("use nada, suspender ou desligar".into()),
    }
}

fn state_label(state: State) -> &'static str {
    match state {
        State::Pending => "aguardando",
        State::Queued => "na fila",
        State::Running => "baixando",
        State::Paused => "pausado",
        State::Completed => "concluído",
        State::Failed => "erro",
    }
}

/// What is wrong with a finished download's check, if anything.
fn check_problem(item: &DownloadInfo) -> Option<String> {
    match (&item.hash, item.checksum_matches()) {
        (Some(Hashing::Failed { error }), _) => Some(format!("não foi possível conferir a soma: {error}")),
        (Some(Hashing::Done { digest }), Some(false)) => Some(format!(
            "o {} não confere: o arquivo pode estar corrompido ou ter sido trocado\n  esperado: {}\n  arquivo:  {}",
            digest.algo.label(),
            item.checksum.as_ref().map_or("", |c| c.hex.as_str()),
            digest.hex
        )),
        _ => None,
    }
}

/// Checks the file of a download (`Request::Verify`) and waits for the result.
async fn verify(id: u64, checksum: Option<Checksum>) -> anyhow::Result<()> {
    let Response::Downloads { items } = fisga_ipc::call(&Request::List).await? else {
        bail!("resposta inesperada do daemon");
    };
    let item = items.into_iter().find(|item| item.id == id).with_context(|| format!("download #{id} não encontrado"))?;
    // Subscribed before asking (the list arrives once it is), so no update is missed.
    let mut updates = Connection::open().await?;
    updates.send(&Request::Subscribe { confirmer: false }).await?;
    while !matches!(updates.recv().await?.context("o daemon encerrou a conexão")?, Response::Downloads { .. }) {}
    fisga_ipc::call(&Request::Verify { id, checksum }).await?;
    if item.state != State::Completed {
        println!("download #{id}: a soma será conferida quando ele terminar");
        return Ok(());
    }
    let bar = ProgressBar::new(item.total.unwrap_or(item.downloaded));
    bar.set_style(bar_style());
    bar.set_message("conferindo");
    let mut started = false;
    while let Some(message) = updates.recv().await? {
        let item = match message {
            Response::Update { item } if item.id == id => item,
            Response::Removed { id: removed } if removed == id => bail!("o download foi removido"),
            _ => continue,
        };
        match &item.hash {
            Some(Hashing::Running { done }) => {
                started = true;
                bar.set_position(*done);
            }
            Some(Hashing::Done { digest }) if started => {
                bar.finish_and_clear();
                if let Some(problem) = check_problem(&item) {
                    bail!(problem);
                }
                println!("{}: {}", digest.algo.label(), digest.hex);
                if item.checksum_matches() == Some(true) {
                    println!("✓ confere com a soma da página");
                }
                return Ok(());
            }
            Some(Hashing::Failed { .. }) if started => {
                bar.finish_and_clear();
                bail!(check_problem(&item).unwrap_or_default());
            }
            _ => {}
        }
    }
    bail!("o daemon encerrou a conexão")
}

fn row(item: &DownloadInfo) -> String {
    let progress = match item.total {
        Some(total) if total > 0 => format!(
            "{:>5.1}% de {}",
            item.downloaded as f64 * 100.0 / total as f64,
            bytes(total)
        ),
        _ => bytes(item.downloaded),
    };
    let speed = if item.state == State::Running { format!("{}/s", bytes(item.speed as u64)) } else { String::new() };
    let mut line = format!("{:>4}  {:<10} {:<20} {:>11}  {}", item.id, state_label(item.state), progress, speed, item.name);
    if item.speed_limit > 0 {
        line.push_str(&format!("  (máx. {}/s)", bytes(item.speed_limit)));
    }
    if let Some(error) = &item.error {
        line.push_str(&format!("\n      └ {error}"));
    }
    if item.awaiting_url {
        line.push_str("\n      └ aguardando novo endereço: clique no link de novo no navegador");
    }
    match (&item.hash, item.checksum_matches()) {
        (Some(Hashing::Running { done }), _) => line.push_str(&format!("\n      └ conferindo a soma… {}", bytes(*done))),
        (_, Some(true)) => line.push_str("\n      └ soma conferida: é o arquivo da página"),
        _ => {}
    }
    if let Some(problem) = check_problem(item) {
        line.push_str(&format!("\n      └ {}", problem.lines().next().unwrap_or_default()));
    }
    line
}

/// In queue order; downloads outside the main queue show theirs.
async fn list() -> anyhow::Result<()> {
    let Response::Downloads { mut items } = fisga_ipc::call(&Request::List).await? else {
        bail!("resposta inesperada do daemon");
    };
    if items.is_empty() {
        println!("nenhum download");
    }
    let settings = get_settings().await?;
    items.sort_by_key(|item| (item.order, item.id));
    for item in &items {
        let mut line = row(item);
        if item.queue != MAIN_QUEUE {
            let at = line.find('\n').unwrap_or(line.len());
            line.insert_str(at, &format!("  [{}]", settings.queue_name(item.queue)));
        }
        println!("{line}");
    }
    Ok(())
}

async fn simple(request: Request) -> anyhow::Result<()> {
    fisga_ipc::call(&request).await?;
    Ok(())
}

async fn watch() -> anyhow::Result<()> {
    let mut connection = Connection::open().await?;
    connection.send(&Request::Subscribe { confirmer: false }).await?;
    while let Some(message) = connection.recv().await? {
        match message {
            Response::Downloads { items } => items.iter().for_each(|i| println!("{}", row(i))),
            Response::Update { item } => println!("{}", row(&item)),
            Response::Removed { id } => println!("{id:>4}  removido"),
            other => println!("{}", serde_json::to_string(&other)?),
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn schedule_arguments() {
        assert_eq!(parse_days("seg, qua,SEX").unwrap(), vec![1, 3, 5]);
        assert_eq!(parse_days("sab,dom").unwrap(), vec![0, 6]);
        assert_eq!(parse_days("todos").unwrap().len(), 7);
        assert!(parse_days("segunda").is_err());
        assert_eq!(parse_stop("nenhum").unwrap(), Stop(None));
        assert_eq!(parse_stop("06:00").unwrap(), Stop(TimeOfDay::new(6, 0)));
        assert!(parse_time("6h").is_err());
        assert_eq!(parse_when_done("desligar").unwrap(), WhenDone::Poweroff);
        assert_eq!(parse_categories("Vídeos, documento").unwrap(), [Category::Video, Category::Document]);
        assert_eq!(parse_categories("musicas,imagens").unwrap(), [Category::Music, Category::Image]);
        assert!(parse_categories("planilha").is_err());
    }
}
