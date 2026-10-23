//! IDM's scheduler: opens and closes the queue at the configured times, and
//! suspends or turns off the computer when a scheduled queue is done.
//!
//! The clock is checked every few seconds (`tick_interval`) instead of
//! sleeping until the next start, so a suspended machine or a changed clock
//! never leaves the queue in the wrong state.
//!
//! Every queue has its own schedule and state.
//!
//! A window only "finishes" (and suspends or powers off) when its start was
//! seen: the clock crossed it, or the user set the schedule while inside it.
//! Booting or restarting `ldmd` in the middle of a window never does, and
//! what was started and finished is kept in `scheduler.json`, so a restart
//! does not repeat the power off.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use anyhow::bail;
use ldm_ipc::{LocalTime, PowerPlan, Response, Schedule, Settings, State as JobState, WhenDone, MAIN_QUEUE};
use serde::{Deserialize, Serialize};

use super::{now, write_json, Manager};
use crate::notify;

/// How often the clock is checked (`$LDM_SCHEDULER_TICK_MS`, for tests).
pub fn tick_interval() -> Duration {
    let ms = std::env::var("LDM_SCHEDULER_TICK_MS").ok().and_then(|v| v.parse().ok()).unwrap_or(15_000);
    Duration::from_millis(ms)
}

/// Windows are identified by the local day they started on.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Saved {
    /// The last window whose start was seen.
    started: Option<i64>,
    /// The last window whose queue finished.
    done: Option<i64>,
}

pub fn saved_path(store: &Path) -> PathBuf {
    store.with_file_name("scheduler.json")
}

/// Per queue. The file of the first version had only the main queue's `Saved`.
pub fn load_saved(path: &Path) -> HashMap<u64, Saved> {
    let Ok(data) = std::fs::read(path) else { return HashMap::new() };
    serde_json::from_slice(&data)
        .or_else(|_| serde_json::from_slice::<Saved>(&data).map(|main| HashMap::from([(MAIN_QUEUE, main)])))
        .unwrap_or_default()
}

/// The scheduler state of every queue.
pub struct Scheduler {
    pub queues: HashMap<u64, State>,
    /// A queue finished with "suspend"/"power off", waiting for the other
    /// queues' downloads to end.
    power_wanted: Option<WhenDone>,
    /// The user was told the power action waits for the other downloads.
    waiting_for_others: bool,
}

impl Scheduler {
    pub fn new(settings: &Settings, now: LocalTime, mut saved: HashMap<u64, Saved>) -> Self {
        let queues = settings
            .all_queues()
            .iter()
            .map(|queue| (queue.id, State::new(&queue.schedule, now, saved.remove(&queue.id).unwrap_or_default())))
            .collect();
        Self { queues, power_wanted: None, waiting_for_others: false }
    }

    pub fn completed(&mut self, queue: u64) {
        if let Some(state) = self.queues.get_mut(&queue) {
            state.completed = true;
        }
    }

    fn saved(&self) -> HashMap<u64, Saved> {
        self.queues.iter().map(|(id, state)| (*id, state.saved)).collect()
    }
}

pub struct State {
    /// Queued downloads may start.
    open: bool,
    /// The scheduled window we are in.
    window: Option<i64>,
    saved: Saved,
    /// A download the queue started completed during this window.
    pub completed: bool,
}

impl State {
    pub fn new(schedule: &Schedule, now: LocalTime, saved: Saved) -> Self {
        let window = schedule.enabled.then(|| schedule.window_at(now)).flatten();
        let mut state = Self { open: false, window, saved, completed: false };
        state.open = state.is_open(schedule);
        state
    }

    fn is_open(&self, schedule: &Schedule) -> bool {
        match (schedule.enabled, self.window, schedule.stop) {
            (false, _, _) => true,
            (true, None, _) => false,
            (true, Some(_), Some(_)) => true,
            // Without a stop time the window runs from its start until its queue is done.
            (true, window, None) => self.saved.started == window && self.saved.done != window,
        }
    }
}

impl Manager {
    /// Queued downloads of `queue` may start (always, unless its scheduler is
    /// on and we are outside its window).
    pub fn queue_open(&self, queue: u64) -> bool {
        self.scheduler.lock().unwrap().queues.get(&queue).map_or(true, |state| state.open)
    }

    /// Downloads waiting in the queue, or running or paused ones the queue
    /// started ("Pausar todos" does not finish the queue).
    fn queue_busy(&self, queue: u64) -> bool {
        self.inner.lock().unwrap().jobs.values().any(|job| {
            job.info.queue == queue
                && (job.info.state == JobState::Queued
                    || (matches!(job.info.state, JobState::Running | JobState::Paused) && job.from_queue))
        })
    }

    pub fn scheduler_tick(self: &Arc<Self>, now: LocalTime) {
        self.scheduler_check(now, &[]);
    }

    /// `changed`: queues whose schedule the user just set, so being inside a
    /// window counts as its start.
    pub fn scheduler_check(self: &Arc<Self>, now: LocalTime, changed: &[u64]) {
        let queues = self.settings().all_queues();
        let mut events = Vec::new();
        let saved = {
            // Lock order: `scheduler`, then `inner` (never the other way around).
            let mut scheduler = self.scheduler.lock().unwrap();
            let before = scheduler.saved();
            scheduler.queues.retain(|id, _| queues.iter().any(|q| q.id == *id));
            for queue in &queues {
                let schedule = &queue.schedule;
                let state = scheduler
                    .queues
                    .entry(queue.id)
                    .or_insert_with(|| State::new(schedule, now, Saved::default()));
                let window = schedule.enabled.then(|| schedule.window_at(now)).flatten();
                let ended = state.window.is_some() && state.window != window;
                if state.window != window {
                    state.window = window;
                    state.completed = false;
                    if window.is_some() {
                        state.saved.started = window;
                    }
                }
                if changed.contains(&queue.id) && window.is_some() {
                    state.saved.started = window;
                }
                let mut finished = false;
                if let Some(key) = window {
                    let ready = state.saved.started == Some(key) && state.saved.done != Some(key) && state.completed;
                    if ready && !self.queue_busy(queue.id) {
                        state.saved.done = Some(key);
                        state.completed = false;
                        finished = true;
                    }
                }
                let open = state.is_open(schedule);
                let opened = open && !state.open;
                state.open = open;
                // Only the end of a window pauses downloads: turning the scheduler
                // on outside its hours lets the running ones finish.
                events.push((queue.clone(), opened, ended && !open && !finished, finished));
            }
            let saved = scheduler.saved();
            (saved != before).then_some(saved)
        };
        if let Some(saved) = saved {
            if let Err(e) = write_json(&self.scheduler_path, &saved, 0o600) {
                tracing::warn!(error = %e, "não foi possível salvar o estado do agendador");
            }
        }
        let mut start = false;
        for (queue, opened, stopped, finished) in events {
            let name = &queue.name;
            if opened && queue.schedule.enabled {
                tracing::info!(queue = queue.id, "agendador: a fila começou");
                notify::send("Agendador", &format!("A fila \u{201c}{name}\u{201d} começou."));
            }
            start |= opened;
            if stopped {
                tracing::info!(queue = queue.id, "agendador: horário encerrado");
                if self.requeue_running(queue.id) > 0 {
                    notify::send(
                        "Agendador",
                        &format!("O horário da fila \u{201c}{name}\u{201d} acabou: os downloads voltaram para ela."),
                    );
                }
            }
            if finished {
                tracing::info!(queue = queue.id, when_done = ?queue.schedule.when_done, "agendador: a fila terminou");
                match queue.schedule.when_done {
                    WhenDone::Nothing => {
                        notify::send("Fila concluída", &format!("Os downloads da fila \u{201c}{name}\u{201d} terminaram."))
                    }
                    action => {
                        let mut scheduler = self.scheduler.lock().unwrap();
                        // Powering off wins over suspending.
                        scheduler.power_wanted = scheduler.power_wanted.max(Some(action));
                    }
                }
            }
        }
        if start {
            self.schedule();
        }
        self.power_when_idle(&queues);
    }

    /// Plans the wanted suspend/power off once no download is running or about
    /// to start (queued in an open queue); until then, says it is waiting.
    fn power_when_idle(self: &Arc<Self>, queues: &[ldm_ipc::Queue]) {
        let Some(action) = self.scheduler.lock().unwrap().power_wanted else { return };
        let open: Vec<u64> = queues.iter().map(|q| q.id).filter(|id| self.queue_open(*id)).collect();
        let busy = self.inner.lock().unwrap().jobs.values().any(|job| {
            job.info.state == JobState::Running || (job.info.state == JobState::Queued && open.contains(&job.info.queue))
        });
        let mut scheduler = self.scheduler.lock().unwrap();
        if busy {
            if !std::mem::replace(&mut scheduler.waiting_for_others, true) {
                let what = if action == WhenDone::Suspend { "suspenso" } else { "desligado" };
                notify::send(
                    "Fila concluída",
                    &format!("O computador será {what} quando os outros downloads terminarem."),
                );
            }
            return;
        }
        scheduler.power_wanted = None;
        scheduler.waiting_for_others = false;
        drop(scheduler);
        self.plan_power(action);
    }

    /// Pauses the downloads the queue started; they wait in the queue again.
    fn requeue_running(&self, queue: u64) -> usize {
        let mut inner = self.inner.lock().unwrap();
        let mut count = 0;
        for job in inner
            .jobs
            .values_mut()
            .filter(|j| j.info.queue == queue && j.info.state == JobState::Running && j.from_queue)
        {
            job.requeue = true;
            if let Some(cancel) = &job.cancel {
                cancel.cancel();
            }
            count += 1;
        }
        count
    }

    pub fn power_plan(&self) -> Option<PowerPlan> {
        self.power.lock().unwrap().clone()
    }

    /// Suspends or turns off the computer after `power_delay`, unless cancelled.
    fn plan_power(self: &Arc<Self>, action: WhenDone) {
        let plan = PowerPlan { action, at: now() + self.power_delay.as_secs() };
        *self.power.lock().unwrap() = Some(plan.clone());
        let _ = self.updates.send(Response::Power { plan: Some(plan.clone()) });
        let what = if action == WhenDone::Suspend { "suspenso" } else { "desligado" };
        notify::send(
            "Fila concluída",
            &format!(
                "O computador será {what} em {} s. Para cancelar, use a janela do LDM ou \u{201c}ldm schedule --cancel\u{201d}.",
                self.power_delay.as_secs()
            ),
        );
        let this = self.clone();
        tokio::spawn(async move {
            tokio::time::sleep(this.power_delay).await;
            let due = {
                let mut power = this.power.lock().unwrap();
                match power.as_ref() == Some(&plan) {
                    true => power.take(),
                    false => None,
                }
            };
            if due.is_some() {
                let _ = this.updates.send(Response::Power { plan: None });
                this.run_power(action).await;
            }
        });
    }

    pub fn cancel_power(&self) -> anyhow::Result<()> {
        if self.power.lock().unwrap().take().is_none() {
            bail!("não há suspensão nem desligamento planejado");
        }
        tracing::info!("desligamento cancelado");
        let _ = self.updates.send(Response::Power { plan: None });
        Ok(())
    }

    async fn run_power(&self, action: WhenDone) {
        let verb = match action {
            WhenDone::Nothing => return,
            WhenDone::Suspend => "suspend",
            WhenDone::Poweroff => "poweroff",
        };
        self.save();
        let mut command = match &self.power_command {
            Some(custom) => {
                let mut command = tokio::process::Command::new("sh");
                command.arg("-c").arg(format!("{custom} {verb}"));
                command
            }
            None => {
                let mut command = tokio::process::Command::new("systemctl");
                command.arg(verb);
                command
            }
        };
        tracing::info!(verb, "agendador: executando");
        match command.stdin(std::process::Stdio::null()).status().await {
            Ok(status) if status.success() => {}
            result => {
                tracing::warn!(?result, verb, "não foi possível suspender/desligar");
                notify::send("Agendador", &format!("Não foi possível executar \u{201c}systemctl {verb}\u{201d}."));
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::super::tests::manager;
    use super::*;
    use ldm_ipc::{AddRequest, TimeOfDay};

    fn at(hour: u16, minute: u16) -> LocalTime {
        // Day 10 is a Monday.
        LocalTime { day: 10, weekday: 1, minutes: hour * 60 + minute }
    }

    /// Changes the schedule without the real-clock tick of `set_settings`.
    fn set_schedule(manager: &Arc<Manager>, start: &str, stop: Option<&str>, when_done: WhenDone) {
        let schedule = Schedule {
            enabled: true,
            start: TimeOfDay::parse(start).unwrap(),
            stop: stop.map(|s| TimeOfDay::parse(s).unwrap()),
            when_done,
            ..Schedule::default()
        };
        manager.settings.lock().unwrap().schedule = schedule;
    }

    /// A server that accepts connections and never answers: downloads stay running.
    async fn silent_server() -> (String, tokio::task::JoinHandle<()>) {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}/big.iso", listener.local_addr().unwrap());
        let task = tokio::spawn(async move {
            let mut held = Vec::new();
            while let Ok((socket, _)) = listener.accept().await {
                held.push(socket);
            }
        });
        (url, task)
    }

    /// Serves a 4-byte file: downloads complete at once.
    async fn file_server() -> String {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}/a.iso", listener.local_addr().unwrap());
        tokio::spawn(async move {
            while let Ok((mut socket, _)) = listener.accept().await {
                tokio::spawn(async move {
                    let mut buffer = [0; 4096];
                    let _ = socket.read(&mut buffer).await;
                    let reply = b"HTTP/1.1 200 OK\r\nContent-Length: 4\r\nConnection: close\r\n\r\ndata";
                    let _ = socket.write_all(reply).await;
                });
            }
        });
        url
    }

    fn add(manager: &Arc<Manager>, url: &str) -> u64 {
        manager.add(AddRequest { url: url.into(), start: true, ..Default::default() }).unwrap()
    }

    async fn wait_for(manager: &Arc<Manager>, id: u64, state: JobState) {
        for _ in 0..100 {
            if manager.state(id) == Some(state) {
                return;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
        panic!("#{id} is {:?}, not {state:?}", manager.state(id));
    }

    #[tokio::test]
    async fn queue_waits_for_the_window_and_returns_to_it_at_the_stop_time() {
        let manager = manager();
        let (url, _server) = silent_server().await;
        set_schedule(&manager, "09:00", Some("17:00"), WhenDone::Nothing);
        manager.scheduler_tick(at(8, 0));
        assert!(!manager.queue_open(MAIN_QUEUE));

        let queued = manager.add(AddRequest { url: url.clone(), start: true, ..Default::default() }).unwrap();
        assert_eq!(manager.state(queued), Some(JobState::Queued), "waits outside the window");
        // "Baixar agora" still starts.
        let now = manager.add(AddRequest { url: url.clone(), confirm: true, ..Default::default() }).unwrap();
        manager.confirm(now, super::super::Answer { now: true, ..Default::default() }).unwrap();
        assert_eq!(manager.state(now), Some(JobState::Running));

        manager.scheduler_tick(at(9, 0));
        assert!(manager.queue_open(MAIN_QUEUE));
        assert_eq!(manager.state(queued), Some(JobState::Running));

        manager.scheduler_tick(at(17, 0));
        assert!(!manager.queue_open(MAIN_QUEUE));
        wait_for(&manager, queued, JobState::Queued).await;
        assert_eq!(manager.state(now), Some(JobState::Running), "started by hand: keeps going");

        // A paused download resumed by hand outside the window starts now.
        manager.pause(now).unwrap();
        wait_for(&manager, now, JobState::Paused).await;
        manager.resume(now).unwrap();
        assert_eq!(manager.state(now), Some(JobState::Running));
        assert_eq!(manager.state(queued), Some(JobState::Queued));
    }

    #[tokio::test]
    async fn turning_the_scheduler_on_lets_running_downloads_finish() {
        let manager = manager();
        let (url, _server) = silent_server().await;
        let id = manager.add(AddRequest { url, start: true, ..Default::default() }).unwrap();
        assert_eq!(manager.state(id), Some(JobState::Running));
        set_schedule(&manager, "09:00", Some("17:00"), WhenDone::Nothing);
        manager.scheduler_tick(at(8, 0));
        assert!(!manager.queue_open(MAIN_QUEUE));
        tokio::time::sleep(Duration::from_millis(200)).await;
        assert_eq!(manager.state(id), Some(JobState::Running));
    }

    #[tokio::test]
    async fn finished_queue_plans_the_power_action_once() {
        let manager = manager();
        let mut updates = manager.subscribe();
        let url = file_server().await;
        set_schedule(&manager, "09:00", None, WhenDone::Suspend);
        // Mondays only: Sunday's window (until Monday 09:00) does not exist.
        manager.settings.lock().unwrap().schedule.days = vec![1];
        manager.scheduler_tick(at(8, 59));
        assert!(!manager.queue_open(MAIN_QUEUE));
        let id = add(&manager, &url);
        manager.scheduler_tick(at(9, 0));
        wait_for(&manager, id, JobState::Completed).await;
        assert!(manager.power_plan().is_none());
        manager.scheduler_tick(at(9, 1));
        let plan = manager.power_plan().expect("planned");
        assert_eq!(plan.action, WhenDone::Suspend);
        assert!(!manager.queue_open(MAIN_QUEUE), "without a stop time the window closes when the queue is done");
        let mut seen = false;
        while let Ok(update) = updates.try_recv() {
            seen |= matches!(update, Response::Power { plan: Some(_) });
        }
        assert!(seen, "subscribers are told");

        manager.cancel_power().unwrap();
        assert!(manager.power_plan().is_none());
        assert!(manager.cancel_power().is_err());
        // Same window: not planned again, not even after a restart.
        manager.scheduler_tick(at(9, 2));
        assert!(manager.power_plan().is_none());
        let saved = load_saved(&manager.scheduler_path);
        assert_eq!(saved[&MAIN_QUEUE], Saved { started: Some(10), done: Some(10) });
        let schedule = manager.settings().schedule;
        assert!(!State::new(&schedule, at(14, 0), saved[&MAIN_QUEUE]).open, "a finished open-ended window stays closed");
    }

    #[test]
    fn first_version_state_file_still_loads() {
        let path = std::env::temp_dir().join(format!("ldm-scheduler-{}.json", std::process::id()));
        std::fs::write(&path, r#"{"started":3,"done":3}"#).unwrap();
        assert_eq!(load_saved(&path)[&MAIN_QUEUE], Saved { started: Some(3), done: Some(3) });
        std::fs::remove_file(&path).unwrap();
    }

    #[tokio::test]
    async fn each_queue_has_its_own_schedule() {
        let manager = manager();
        let (url, _server) = silent_server().await;
        let night = ldm_ipc::Queue {
            id: 1,
            name: "Noite".into(),
            max_parallel: 2,
            schedule: Schedule {
                enabled: true,
                start: TimeOfDay::parse("23:00").unwrap(),
                stop: TimeOfDay::parse("06:00"),
                ..Schedule::default()
            },
        };
        manager.settings.lock().unwrap().set_queue(night);
        manager.scheduler_tick(at(12, 0));
        assert!(manager.queue_open(MAIN_QUEUE) && !manager.queue_open(1));
        let main = add(&manager, &url);
        let later = manager.add(AddRequest { url: url.clone(), start: true, queue: Some(1), ..Default::default() }).unwrap();
        assert_eq!(manager.state(main), Some(JobState::Running));
        assert_eq!(manager.state(later), Some(JobState::Queued));
        manager.scheduler_tick(at(23, 0));
        assert_eq!(manager.state(later), Some(JobState::Running));
    }

    #[tokio::test]
    async fn power_off_waits_for_the_other_queues() {
        let manager = manager();
        let (slow, _server) = silent_server().await;
        let fast = file_server().await;
        let night = ldm_ipc::Queue {
            id: 1,
            name: "Noite".into(),
            max_parallel: 1,
            schedule: Schedule {
                enabled: true,
                start: TimeOfDay::parse("09:00").unwrap(),
                stop: TimeOfDay::parse("17:00"),
                when_done: WhenDone::Poweroff,
                ..Schedule::default()
            },
        };
        manager.settings.lock().unwrap().set_queue(night);
        manager.scheduler_tick(at(8, 0));
        let other = add(&manager, &slow);
        let id = manager.add(AddRequest { url: fast, start: true, queue: Some(1), ..Default::default() }).unwrap();
        manager.scheduler_tick(at(9, 0));
        wait_for(&manager, id, JobState::Completed).await;
        manager.scheduler_tick(at(9, 1));
        assert!(manager.power_plan().is_none(), "the main queue is still downloading");
        manager.pause(other).unwrap();
        wait_for(&manager, other, JobState::Paused).await;
        manager.scheduler_tick(at(9, 2));
        assert_eq!(manager.power_plan().map(|p| p.action), Some(WhenDone::Poweroff));
    }

    #[tokio::test]
    async fn starting_in_the_middle_of_a_window_never_powers_off() {
        let manager = manager();
        let url = file_server().await;
        set_schedule(&manager, "09:00", Some("17:00"), WhenDone::Poweroff);
        // As if ldmd started at 10:00 (the machine was booted then).
        let state = State::new(&manager.settings().schedule, at(10, 0), Saved::default());
        manager.scheduler.lock().unwrap().queues.insert(MAIN_QUEUE, state);
        assert!(manager.queue_open(MAIN_QUEUE), "a window with a stop time is open");
        let id = add(&manager, &url);
        wait_for(&manager, id, JobState::Completed).await;
        manager.scheduler_tick(at(10, 1));
        assert!(manager.power_plan().is_none());

        // Open-ended: closed until the next start.
        set_schedule(&manager, "09:00", None, WhenDone::Poweroff);
        assert!(!State::new(&manager.settings().schedule, at(10, 0), Saved::default()).open);
    }

    #[tokio::test]
    async fn setting_the_schedule_inside_its_window_counts_as_its_start() {
        let manager = manager();
        let url = file_server().await;
        set_schedule(&manager, "09:00", Some("17:00"), WhenDone::Suspend);
        manager.scheduler_check(at(10, 0), &[MAIN_QUEUE]);
        let id = add(&manager, &url);
        wait_for(&manager, id, JobState::Completed).await;
        manager.scheduler_tick(at(10, 1));
        assert!(manager.power_plan().is_some());
    }

    #[tokio::test]
    async fn pausing_everything_does_not_finish_the_queue() {
        let manager = manager();
        let (slow, _server) = silent_server().await;
        let fast = file_server().await;
        set_schedule(&manager, "09:00", Some("17:00"), WhenDone::Poweroff);
        manager.scheduler_tick(at(8, 0));
        let done = add(&manager, &fast);
        let paused = add(&manager, &slow);
        manager.scheduler_tick(at(9, 0));
        wait_for(&manager, done, JobState::Completed).await;
        manager.pause(paused).unwrap();
        wait_for(&manager, paused, JobState::Paused).await;
        manager.scheduler_tick(at(9, 1));
        assert!(manager.power_plan().is_none(), "a paused download of the queue is still pending");
        manager.remove(paused, false).await.unwrap();
        manager.scheduler_tick(at(9, 2));
        assert!(manager.power_plan().is_some());
    }

    #[tokio::test]
    async fn planned_power_action_runs_after_the_delay() {
        let manager = manager();
        let marker = manager.store.with_file_name("power");
        let mut manager = Arc::try_unwrap(manager).ok().expect("only reference");
        manager.power_delay = Duration::from_millis(100);
        manager.power_command = Some(format!("echo >{}", marker.display()));
        let manager = Arc::new(manager);
        manager.plan_power(WhenDone::Poweroff);
        for _ in 0..50 {
            if std::fs::read_to_string(&marker).is_ok_and(|s| s.trim() == "poweroff") {
                assert!(manager.power_plan().is_none());
                return;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
        panic!("power command did not run");
    }

    #[test]
    fn initial_state_follows_the_clock() {
        let schedule = Schedule {
            enabled: true,
            start: TimeOfDay::parse("09:00").unwrap(),
            stop: TimeOfDay::parse("17:00"),
            ..Schedule::default()
        };
        assert!(!State::new(&schedule, at(8, 0), Saved::default()).open);
        assert!(State::new(&schedule, at(10, 0), Saved::default()).open);
        assert!(State::new(&Schedule::default(), at(8, 0), Saved::default()).open, "scheduler off: always open");
    }
}
