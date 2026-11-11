#![cfg_attr(all(windows, not(debug_assertions)), windows_subsystem = "windows")]

mod demo;
mod instance;
#[cfg(windows)]
mod ptt;
mod selfcheck;
#[cfg(windows)]
mod tray;

use std::process::ExitCode;
use std::sync::Arc;
use std::time::Duration;

use dn_cache::bus::{self, Command, UiLink};
use dn_cache::{Store, StoreLimits};
use dn_settings::{Paths, Settings};
use dn_ui::{APP_TITLE, DiscordApp, UiOptions};

pub const ICON_RGBA: &[u8] = include_bytes!("../../../assets/icon64.rgba");

#[derive(Debug, Default)]
struct Args {
    version: bool,
    help: bool,
    minimized: bool,
    demo: bool,
    self_check: bool,
    network: bool,
    software: bool,
}

fn parse_args() -> Result<Args, String> {
    let mut a = Args::default();
    for arg in std::env::args().skip(1) {
        match arg.as_str() {
            "--version" | "-V" => a.version = true,
            "--help" | "-h" => a.help = true,
            "--minimized" => a.minimized = true,
            "--demo" => a.demo = true,
            "--self-check" => a.self_check = true,
            "--network" => a.network = true,
            "--software-render" => a.software = true,
            other => return Err(format!("unknown argument: {other}")),
        }
    }
    Ok(a)
}

const HELP: &str = "Discord Native — native desktop client for Discord (official APIs only)

USAGE: discord-native [OPTIONS]

  --minimized        start hidden in the tray (used by start-on-boot)
  --demo             offline demo data, no Discord connection (UI preview / testing)
  --self-check       check local subsystems (paths, database, credential store, audio) and exit
  --network          with --self-check: also check that Discord's API and gateway are reachable
  --software-render  use the OpenGL renderer instead of wgpu
  -V, --version      print version
  -h, --help         print this help

Environment: DN_HOME=<dir> runs in portable mode with all data under <dir>.";

fn main() -> ExitCode {
    let args = match parse_args() {
        Ok(a) => a,
        Err(e) => {
            eprintln!("{e}\n\n{HELP}");
            return ExitCode::from(2);
        }
    };
    if args.help {
        println!("{HELP}");
        return ExitCode::SUCCESS;
    }
    if args.version {
        println!("discord-native {}", env!("CARGO_PKG_VERSION"));
        return ExitCode::SUCCESS;
    }
    let paths = Paths::detect();
    if let Err(e) = paths.ensure() {
        eprintln!("cannot create data directories: {e}");
        return ExitCode::FAILURE;
    }
    if args.self_check {
        return selfcheck::run(&paths, args.network);
    }

    let instance = match instance::acquire(&paths) {
        instance::Instance::Primary(listener) => Some(listener),
        instance::Instance::AlreadyRunning => {
            eprintln!("Discord Native is already running; asked it to show its window.");
            return ExitCode::SUCCESS;
        }
        instance::Instance::Unavailable => None,
    };

    let settings = Settings::load(&paths.settings_file());
    let _log = match dn_diagnostics::init_logging(&paths.logs_dir, &settings.advanced.log_level, cfg!(debug_assertions)) {
        Ok(g) => Some(g),
        Err(e) => {
            eprintln!("logging unavailable: {e}");
            None
        }
    };
    dn_diagnostics::install_panic_hook(paths.crash_dir());
    let (marker, unclean) = dn_diagnostics::SessionMarker::begin(&paths.data_dir);
    tracing::info!(version = env!("CARGO_PKG_VERSION"), os = std::env::consts::OS, demo = args.demo, "starting");

    let rt = match tokio::runtime::Builder::new_multi_thread().worker_threads(2).thread_name("dn-worker").enable_all().build() {
        Ok(rt) => rt,
        Err(e) => {
            tracing::error!("tokio runtime: {e}");
            return ExitCode::FAILURE;
        }
    };

    let store = Store::shared(StoreLimits {
        inactive_channel_messages: settings.advanced.memory_messages_per_channel as usize,
        ..Default::default()
    });
    let shared_settings = Arc::new(parking_lot::RwLock::new(settings.clone()));
    let (ui_link, backend_link, cmd_rx) = bus::link(store.clone(), shared_settings.clone());

    let media = {
        let repaint = ui_link.repaint.clone();
        let cfg = dn_media::MediaConfig {
            disk_dir: Some(paths.image_cache_dir()),
            disk_budget_bytes: settings.advanced.disk_cache_mb as u64 * 1024 * 1024,
            ..Default::default()
        };
        dn_media::MediaLoader::new(cfg, rt.handle().clone(), Arc::new(move || repaint.now()))
    };

    let backend = if args.demo {
        demo::populate(&store);
        rt.spawn(demo::serve(backend_link, cmd_rx));
        None
    } else {
        Some(dn_client::start(rt.handle(), dn_client::BackendConfig::production(paths.clone()), backend_link, cmd_rx))
    };

    let notice = unclean.then(|| {
        let detail = dn_diagnostics::latest_crash_report(&paths.crash_dir())
            .map(|(p, _)| format!(" A crash report was saved to {}.", p.display()))
            .unwrap_or_default();
        format!("Discord Native didn't shut down cleanly last time.{detail}")
    });

    let has_tray = cfg!(windows);
    let start_hidden = has_tray && (args.minimized || settings.behavior.start_minimized);
    let use_wgpu = settings.behavior.hardware_acceleration && !args.software;

    #[cfg(windows)]
    ptt::spawn(ui_link.clone());

    let launch = |renderer: eframe::Renderer| {
        let opts = native_options(&settings, renderer, start_hidden);
        let link = ui_link.clone();
        let media = media.clone();
        let listener = instance.as_ref().and_then(|l| l.try_clone().ok());
        let notice = notice.clone();
        eframe::run_native(
            APP_TITLE,
            opts,
            Box::new(move |cc| {
                let app = DiscordApp::new(
                    &cc.egui_ctx,
                    link.clone(),
                    UiOptions {
                        media: Some(media),
                        texture_budget_bytes: link.settings.read().advanced.image_cache_mb as usize * 1024 * 1024,
                        use_system_fonts: true,
                        has_tray,
                        global_ptt: cfg!(windows),
                    },
                );
                Ok(Box::new(Shell::new(app, link, &cc.egui_ctx, listener, notice)))
            }),
        )
    };

    let mut result = launch(if use_wgpu { eframe::Renderer::Wgpu } else { eframe::Renderer::Glow });
    if let Err(e) = &result {
        if use_wgpu {
            tracing::warn!("wgpu renderer failed ({e}); retrying with OpenGL");
            result = launch(eframe::Renderer::Glow);
        }
    }

    ui_link.send(Command::Shutdown);
    if let Some(h) = backend {
        let _ = rt.block_on(async { tokio::time::timeout(Duration::from_secs(6), h).await });
    }
    rt.shutdown_timeout(Duration::from_secs(1));
    marker.end();
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            tracing::error!("UI failed to start: {e}");
            eprintln!("Discord Native could not open its window: {e}");
            ExitCode::FAILURE
        }
    }
}

fn native_options(s: &Settings, renderer: eframe::Renderer, hidden: bool) -> eframe::NativeOptions {
    let icon = egui::IconData { rgba: ICON_RGBA.to_vec(), width: 64, height: 64 };
    let viewport = egui::ViewportBuilder::default()
        .with_title(APP_TITLE)
        .with_app_id("discord-native")
        .with_inner_size([s.window.width.max(940.0), s.window.height.max(500.0)])
        .with_min_inner_size([940.0, 500.0])
        .with_maximized(s.window.maximized)
        .with_visible(!hidden)
        .with_icon(Arc::new(icon))
        .with_drag_and_drop(true);
    eframe::NativeOptions { viewport, renderer, vsync: true, ..Default::default() }
}

/// Wraps the UI with shell concerns: tray, single-instance activation, window state.
struct Shell {
    app: DiscordApp,
    link: UiLink,
    events: crossbeam_channel::Receiver<ShellEvent>,
    #[cfg(windows)]
    _tray: Option<tray::Tray>,
    last_size: Option<(f32, f32, bool)>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ShellEvent {
    Show,
    Quit,
}

impl Shell {
    fn new(mut app: DiscordApp, link: UiLink, ctx: &egui::Context, listener: Option<std::net::TcpListener>, notice: Option<String>) -> Self {
        let (tx, rx) = crossbeam_channel::unbounded();
        if let Some(l) = listener {
            instance::serve(l, tx.clone(), ctx.clone());
        }
        #[cfg(windows)]
        let tray = tray::create(ctx, tx.clone());
        if let Some(text) = notice {
            app.st.toast(text, true);
        }
        let _ = tx;
        Shell {
            app,
            link,
            events: rx,
            #[cfg(windows)]
            _tray: tray,
            last_size: None,
        }
    }
}

impl eframe::App for Shell {
    fn logic(&mut self, ctx: &egui::Context, frame: &mut eframe::Frame) {
        while let Ok(ev) = self.events.try_recv() {
            match ev {
                ShellEvent::Show => {
                    ctx.send_viewport_cmd(egui::ViewportCommand::Visible(true));
                    ctx.send_viewport_cmd(egui::ViewportCommand::Minimized(false));
                    ctx.send_viewport_cmd(egui::ViewportCommand::Focus);
                }
                ShellEvent::Quit => {
                    self.app.request_quit();
                    ctx.send_viewport_cmd(egui::ViewportCommand::Close);
                }
            }
        }
        let vp = ctx.input(|i| (i.viewport().inner_rect, i.viewport().maximized));
        if let (Some(r), Some(max)) = vp {
            self.last_size = Some((r.width(), r.height(), max));
        }
        self.app.logic(ctx, frame);
    }

    fn ui(&mut self, ui: &mut egui::Ui, frame: &mut eframe::Frame) {
        self.app.ui(ui, frame);
    }

    fn on_exit(&mut self, gl: Option<&eframe::glow::Context>) {
        if let Some((w, h, max)) = self.last_size {
            let mut s = self.link.settings.write();
            s.window.maximized = max;
            if !max {
                s.window.width = w;
                s.window.height = h;
            }
        }
        self.app.on_exit(gl);
    }
}
