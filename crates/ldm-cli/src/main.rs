mod hosts;

use std::path::PathBuf;
use std::time::Duration;

use anyhow::{bail, Context};
use clap::{Parser, Subcommand};
use indicatif::{ProgressBar, ProgressStyle};
use ldm_core::{DownloadOptions, EngineEvent, Outcome, RequestSpec, Target};
use ldm_ipc::{AddRequest, Connection, DownloadInfo, Request, Response, State};
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

#[derive(Parser)]
#[command(name = "ldm", version, about = "LDM: gerenciador de downloads para Linux")]
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

#[derive(Subcommand)]
enum Command {
    /// Envia um download para o daemon (ldmd)
    Add {
        #[command(flatten)]
        source: Source,
        /// Acompanha o progresso até terminar
        #[arg(short, long)]
        wait: bool,
        /// Pede confirmação na janela "Novo download" (ldm-gui) antes de começar
        #[arg(long, conflicts_with = "wait")]
        confirm: bool,
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
        #[arg(long)]
        uninstall: bool,
    },
    /// Mostra ou altera as configurações do daemon
    Settings {
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
    },
    /// Encerra o daemon (pausa os downloads em andamento)
    Stop,
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    match Cli::parse().command {
        Command::Add { source, wait, confirm } => add(source, wait, confirm).await,
        Command::Confirm { id, queue, dir, name } => {
            let output_dir = dir.map(|d| std::path::absolute(&d).unwrap_or(d));
            simple(Request::Confirm { id, now: !queue, file_name: name, output_dir }).await
        }
        Command::Get { source } => get(source).await,
        Command::List => list().await,
        Command::Pause { id } => simple(Request::Pause { id }).await,
        Command::Resume { id } => simple(Request::Resume { id }).await,
        Command::Rm { id, delete_file } => simple(Request::Remove { id, delete_file }).await,
        Command::Watch => watch().await,
        Command::InstallHosts { chrome_ids, uninstall } => hosts::install(&chrome_ids, uninstall),
        Command::Settings { dir, category_folders, max_parallel, connections, show_progress, speed_limit } => {
            settings(dir, category_folders, max_parallel, connections, show_progress, speed_limit).await
        }
        Command::Stop => match ldm_ipc::call(&Request::Shutdown).await {
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
    let client = ldm_core::build_client()?;
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
    let download = ldm_core::download(&client, &spec, target, &opts, tx, cancel);
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
        Err(ldm_core::Error::Cancelled) => println!("cancelado"),
        Err(e) => bail!(e),
    }
    Ok(())
}

async fn add(source: Source, wait: bool, confirm: bool) -> anyhow::Result<()> {
    ldm_ipc::ensure_daemon().await?;
    let request = AddRequest {
        url: source.url.clone(),
        output_dir: source.dir.clone().map(|d| std::path::absolute(&d).unwrap_or(d)),
        file_name: source.name.clone(),
        connections: source.connections,
        referrer: source.referer.clone(),
        user_agent: source.user_agent.clone(),
        headers: source.headers()?,
        start: true,
        confirm,
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
        match item.state {
            State::Completed => {
                bar.finish_and_clear();
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

async fn settings(
    dir: Option<PathBuf>,
    category_folders: Option<bool>,
    max_parallel: Option<usize>,
    connections: Option<usize>,
    show_progress: Option<bool>,
    speed_limit: Option<u64>,
) -> anyhow::Result<()> {
    ldm_ipc::ensure_daemon().await?;
    let Response::Settings { settings: mut current } = ldm_ipc::call(&Request::GetSettings).await? else {
        bail!("resposta inesperada do daemon");
    };
    let changed = dir.is_some()
        || category_folders.is_some()
        || max_parallel.is_some()
        || connections.is_some()
        || show_progress.is_some()
        || speed_limit.is_some();
    if let Some(dir) = dir {
        current.download_dir = if dir.as_os_str() == "padrao" { None } else { Some(std::path::absolute(&dir)?) };
    }
    current.category_folders = category_folders.unwrap_or(current.category_folders);
    current.max_parallel = max_parallel.unwrap_or(current.max_parallel);
    current.connections = connections.unwrap_or(current.connections);
    current.show_progress = show_progress.unwrap_or(current.show_progress);
    current.speed_limit = speed_limit.map(|kb| kb * 1024).unwrap_or(current.speed_limit);
    if changed {
        ldm_ipc::call(&Request::SetSettings { settings: current.clone() }).await?;
    }
    let dir = current.download_dir.map(|d| d.display().to_string()).unwrap_or_else(|| "(Downloads do sistema)".into());
    println!("pasta de downloads:      {dir}");
    println!("subpastas por tipo:      {}", if current.category_folders { "sim" } else { "não" });
    println!("downloads ao mesmo tempo: {}", current.max_parallel);
    println!("conexões por arquivo:    {}", current.connections);
    println!("janela de progresso:     {}", if current.show_progress { "sim" } else { "não" });
    match current.speed_limit {
        0 => println!("limite de velocidade:    sem limite"),
        limit => println!("limite de velocidade:    {}/s", bytes(limit)),
    }
    Ok(())
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
    if let Some(error) = &item.error {
        line.push_str(&format!("\n      └ {error}"));
    }
    line
}

async fn list() -> anyhow::Result<()> {
    let Response::Downloads { items } = ldm_ipc::call(&Request::List).await? else {
        bail!("resposta inesperada do daemon");
    };
    if items.is_empty() {
        println!("nenhum download");
    }
    for item in &items {
        println!("{}", row(item));
    }
    Ok(())
}

async fn simple(request: Request) -> anyhow::Result<()> {
    ldm_ipc::call(&request).await?;
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
