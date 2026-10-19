use crate::demo::DemoBattery;
use battery_control::{
    AuthInput, AuthState, Battery, BatteryStatus, Capabilities, Command as Control, DeviceClass,
    DeviceInfo, Discovered, DiscoverOptions, discover,
};
use futures_util::{FutureExt, StreamExt};
use std::sync::mpsc::{Receiver, Sender, channel};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};
use tokio::sync::mpsc::{UnboundedReceiver, UnboundedSender, unbounded_channel};

const POLL: Duration = Duration::from_secs(2);
const STATUS_LIMIT: Duration = Duration::from_secs(12);
const CONNECT_LIMIT: Duration = Duration::from_secs(40);
const COMMAND_LIMIT: Duration = Duration::from_secs(15);
const PAIR_LIMIT: Duration = Duration::from_secs(60);
const FAILURES_BEFORE_DROP: u32 = 3;
const CLOSE_LIMIT: Duration = Duration::from_secs(4);

pub const DEMO_ID: &str = "demo:pack";

pub enum Command {
    Scan { serial: bool },
    Connect(String),
    Disconnect,
    Control(Control),
    Pair(AuthInput),
    ForgetPairing,
    Quit,
}

#[derive(Clone)]
pub struct Row {
    pub id: String,
    pub label: String,
    pub backend: String,
    pub class: DeviceClass,
}

pub enum Update {
    Scanning,
    Found(Vec<Row>),
    Connecting(String),
    Connected { info: DeviceInfo, caps: Capabilities },
    Status(BatteryStatus),
    Pairing(AuthState),
    Notice(String),
    Disconnected,
}

enum Source {
    Real(Box<Discovered>),
    Demo,
}

struct Entry {
    row: Row,
    source: Source,
}

pub struct Worker {
    tx: UnboundedSender<Command>,
    pub rx: Receiver<Update>,
    handle: Option<JoinHandle<()>>,
}

impl Worker {
    pub fn spawn(ctx: egui::Context, demo: bool) -> Result<Self, String> {
        let runtime = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(2)
            .enable_all()
            .build()
            .map_err(|e| e.to_string())?;
        let (tx, commands) = unbounded_channel();
        let (out, rx) = channel();
        let handle = std::thread::Builder::new()
            .name("battery".into())
            .spawn(move || runtime.block_on(run(commands, Out { tx: out, ctx }, demo)))
            .map_err(|e| e.to_string())?;
        Ok(Self { tx, rx, handle: Some(handle) })
    }

    pub fn send(&self, command: Command) {
        let _ = self.tx.send(command);
    }

    pub fn close(&mut self) {
        let _ = self.tx.send(Command::Quit);
        let Some(handle) = self.handle.take() else { return };
        let started = Instant::now();
        while !handle.is_finished() && started.elapsed() < CLOSE_LIMIT {
            std::thread::sleep(Duration::from_millis(20));
        }
        if handle.is_finished() {
            let _ = handle.join();
        }
    }
}

impl Drop for Worker {
    fn drop(&mut self) {
        self.close();
    }
}

struct Out {
    tx: Sender<Update>,
    ctx: egui::Context,
}

impl Out {
    fn send(&self, update: Update) {
        let _ = self.tx.send(update);
        self.ctx.request_repaint();
    }
}

enum Event {
    Command(Option<Command>),
    Poll,
    Failed(String),
}

async fn follow(
    battery: &mut dyn Battery,
    commands: &mut UnboundedReceiver<Command>,
    live: &mut BatteryStatus,
    failures: &mut u32,
    out: &Out,
) -> Event {
    let mut updates = battery.updates(POLL);
    loop {
        tokio::select! {
            c = commands.recv() => return Event::Command(c),
            u = updates.next() => match u {
                Some(Ok(first)) => {
                    *failures = 0;
                    live.apply(&first);
                    while let Some(Some(Ok(more))) = updates.next().now_or_never() {
                        live.apply(&more);
                    }
                    out.send(Update::Status(live.clone()));
                }
                Some(Err(e)) => return Event::Failed(e.to_string()),
                None => return Event::Failed("the device stopped reporting".into()),
            },
        }
    }
}

async fn poll_once(battery: &mut dyn Battery, failures: &mut u32, out: &Out) -> Option<String> {
    match tokio::time::timeout(STATUS_LIMIT, battery.status()).await {
        Ok(Ok(status)) => {
            *failures = 0;
            out.send(Update::Status(status));
            None
        }
        Ok(Err(e)) => Some(e.to_string()),
        Err(_) => Some("no answer from the device".into()),
    }
}

async fn run(mut commands: UnboundedReceiver<Command>, out: Out, demo: bool) {
    let mut found: Vec<Entry> = Vec::new();
    let mut link: Option<Box<dyn Battery>> = None;
    let mut live = BatteryStatus::default();
    let mut failures = 0u32;
    let mut needs_pairing = false;
    loop {
        let event = match link.as_mut() {
            None => Event::Command(commands.recv().await),
            Some(_) if needs_pairing => Event::Command(commands.recv().await),
            Some(battery) if battery.has_stream() => {
                follow(battery.as_mut(), &mut commands, &mut live, &mut failures, &out).await
            }
            Some(_) => tokio::select! {
                c = commands.recv() => Event::Command(c),
                _ = tokio::time::sleep(POLL) => Event::Poll,
            },
        };
        let mut poll = matches!(event, Event::Poll);
        let mut problem = None;
        match event {
            Event::Poll => {}
            Event::Failed(why) => {
                let quiet = why.contains("timed out") && !live.sensors.is_empty();
                if quiet {
                    failures += 1;
                    if failures >= FAILURES_BEFORE_DROP {
                        problem = Some(why);
                    }
                } else {
                    problem = Some(why);
                }
            }
            Event::Command(None | Some(Command::Quit)) => break,
            Event::Command(Some(command)) => match command {
                Command::Quit => break,
                Command::Scan { serial } => {
                    if link.is_none() {
                        found = scan(&out, serial, demo).await;
                    }
                }
                Command::Connect(id) => {
                    drop_link(&mut link).await;
                    live = BatteryStatus::default();
                    link = connect(&out, &found, &id).await;
                    failures = 0;
                    needs_pairing = false;
                    if let Some(battery) = link.as_mut()
                        && battery.capabilities().contains(Capabilities::REQUIRES_AUTH)
                    {
                        needs_pairing =
                            !pair(battery.as_mut(), AuthInput::None, &out).await;
                    }
                    poll = !needs_pairing && link.as_ref().is_some_and(|b| !b.has_stream());
                }
                Command::Disconnect => {
                    drop_link(&mut link).await;
                    out.send(Update::Disconnected);
                }
                Command::Control(control) => {
                    if let Some(battery) = link.as_mut() {
                        match tokio::time::timeout(COMMAND_LIMIT, battery.execute(control)).await {
                            Ok(Ok(())) => {}
                            Ok(Err(e)) => out.send(Update::Notice(e.to_string())),
                            Err(_) => out.send(Update::Notice("the device did not answer".into())),
                        }
                        poll = !battery.has_stream();
                    }
                }
                Command::Pair(input) => {
                    if let Some(battery) = link.as_mut() {
                        needs_pairing = !pair(battery.as_mut(), input, &out).await;
                    }
                }
                Command::ForgetPairing => {
                    if let Some(battery) = link.as_mut() {
                        match battery.forget_auth().await {
                            Ok(()) => {
                                needs_pairing = battery
                                    .capabilities()
                                    .contains(Capabilities::REQUIRES_AUTH);
                                out.send(Update::Notice(
                                    "pairing forgotten on this computer".into(),
                                ));
                            }
                            Err(e) => out.send(Update::Notice(e.to_string())),
                        }
                    }
                }
            },
        }
        if poll
            && problem.is_none()
            && let Some(battery) = link.as_mut()
        {
            problem = poll_once(battery.as_mut(), &mut failures, &out).await;
        }
        if let Some(why) = problem {
            failures += 1;
            if failures >= FAILURES_BEFORE_DROP {
                drop_link(&mut link).await;
                out.send(Update::Notice(format!("connection lost: {why}")));
                out.send(Update::Disconnected);
            } else {
                out.send(Update::Notice(why));
                tokio::time::sleep(Duration::from_millis(500)).await;
            }
        }
    }
    drop_link(&mut link).await;
}

async fn pair(battery: &mut dyn Battery, input: AuthInput, out: &Out) -> bool {
    match tokio::time::timeout(PAIR_LIMIT, battery.authenticate(input)).await {
        Ok(Ok(state)) => {
            let done = state.is_authed();
            out.send(Update::Pairing(state));
            done
        }
        Ok(Err(e)) => {
            out.send(Update::Notice(e.to_string()));
            false
        }
        Err(_) => {
            out.send(Update::Notice("pairing timed out".into()));
            false
        }
    }
}

async fn drop_link(link: &mut Option<Box<dyn Battery>>) {
    if let Some(mut battery) = link.take() {
        let _ = tokio::time::timeout(Duration::from_secs(3), battery.disconnect()).await;
    }
}

async fn scan(out: &Out, serial: bool, demo: bool) -> Vec<Entry> {
    out.send(Update::Scanning);
    let mut entries: Vec<Entry> = Vec::new();
    if demo {
        tokio::time::sleep(Duration::from_millis(600)).await;
        entries.push(Entry {
            row: Row {
                id: DEMO_ID.into(),
                label: "Demo 16S pack".into(),
                backend: "demo".into(),
                class: DeviceClass::Bms,
            },
            source: Source::Demo,
        });
    } else {
        let options = DiscoverOptions { probe_serial: serial, ..Default::default() };
        match discover(&options).await {
            Ok(devices) => {
                for d in devices {
                    entries.push(Entry {
                        row: Row {
                            id: d.id.clone(),
                            label: d.label.clone(),
                            backend: d.backend.to_string(),
                            class: d.class,
                        },
                        source: Source::Real(Box::new(d)),
                    });
                }
            }
            Err(e) => out.send(Update::Notice(format!("scan failed: {e}"))),
        }
    }
    out.send(Update::Found(entries.iter().map(|e| e.row.clone()).collect()));
    entries
}

async fn connect(out: &Out, found: &[Entry], id: &str) -> Option<Box<dyn Battery>> {
    let Some(entry) = found.iter().find(|e| e.row.id == id) else {
        out.send(Update::Notice("scan again, that device is not in the list".into()));
        out.send(Update::Disconnected);
        return None;
    };
    out.send(Update::Connecting(entry.row.label.clone()));
    let opened: Result<Box<dyn Battery>, String> = match &entry.source {
        Source::Demo => Ok(Box::new(DemoBattery::new())),
        Source::Real(device) => {
            match tokio::time::timeout(CONNECT_LIMIT, device.connect(6)).await {
                Ok(Ok(battery)) => Ok(battery),
                Ok(Err(e)) => Err(e.to_string()),
                Err(_) => Err("timed out connecting".into()),
            }
        }
    };
    match opened {
        Ok(battery) => {
            out.send(Update::Connected {
                info: battery.info().clone(),
                caps: battery.capabilities(),
            });
            Some(battery)
        }
        Err(why) => {
            out.send(Update::Notice(format!("could not connect: {why}")));
            out.send(Update::Disconnected);
            None
        }
    }
}
