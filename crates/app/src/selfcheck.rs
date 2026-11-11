//! `--self-check`: verifies local subsystems without opening a window (useful on CI and for
//! support). With `--network` it also checks that Discord's API and gateway are reachable.

use std::process::ExitCode;
use std::time::{Duration, Instant};

use dn_settings::{Paths, Settings};

struct Report {
    failed: bool,
}

impl Report {
    fn check(&mut self, name: &str, r: Result<String, String>) {
        match r {
            Ok(detail) => println!("[ ok ] {name}: {detail}"),
            Err(e) => {
                self.failed = true;
                println!("[FAIL] {name}: {e}");
            }
        }
    }
}

pub fn run(paths: &Paths, network: bool) -> ExitCode {
    println!("Discord Native {} self-check ({} {})", env!("CARGO_PKG_VERSION"), std::env::consts::OS, std::env::consts::ARCH);
    let mut r = Report { failed: false };

    r.check("data directories", {
        let probe = paths.data_dir.join(".write-test");
        std::fs::write(&probe, b"ok")
            .and_then(|_| std::fs::remove_file(&probe))
            .map(|_| paths.data_dir.display().to_string())
            .map_err(|e| e.to_string())
    });

    r.check("settings", {
        let s = Settings::load(&paths.settings_file());
        Ok(format!("theme {:?}, renderer {}", s.appearance.theme, if s.behavior.hardware_acceleration { "wgpu" } else { "OpenGL" }))
    });

    r.check("SQLite + FTS5", {
        let start = Instant::now();
        dn_storage::Storage::open_in_memory()
            .and_then(|mut st| {
                let m = dn_core::Message {
                    id: dn_core::Id(1),
                    channel_id: dn_core::Id(2),
                    content: "self check message".into(),
                    ..Default::default()
                };
                st.upsert_messages(&[m])?;
                st.search(&dn_storage::SearchFilter { query: "check".into(), ..Default::default() })
            })
            .map_err(|e| e.to_string())
            .and_then(|hits| {
                if hits.len() == 1 {
                    Ok(format!("search ok in {:?}", start.elapsed()))
                } else {
                    Err(format!("expected 1 hit, got {}", hits.len()))
                }
            })
    });

    r.check("local message cache", {
        let db = paths.database_file();
        if db.exists() {
            dn_storage::Storage::open(&db)
                .and_then(|s| s.stats())
                .map(|st| format!("{} messages, {} KB", st.messages, st.db_bytes / 1024))
                .map_err(|e| e.to_string())
        } else {
            Ok("not created yet".into())
        }
    });

    r.check("credential store", {
        use dn_auth::SecretStore;
        let store = dn_auth::KeyringStore::new("DiscordNative");
        store.get("self-check-nonexistent").map(|_| "accessible".to_string()).map_err(|e| e.to_string())
    });

    let accounts = dn_auth::AccountManager::load(paths.accounts_file(), std::sync::Arc::new(dn_auth::MemoryStore::default()))
        .map(|m| m.accounts().len())
        .map_err(|e| e.to_string());
    r.check("accounts file", accounts.map(|n| format!("{n} saved account(s)")));

    let (inputs, outputs) = (dn_audio::input_devices(), dn_audio::output_devices());
    println!("[info] audio: {} input / {} output device(s)", inputs.len(), outputs.len());
    for d in inputs.iter().chain(outputs.iter()) {
        println!("       - {}{}", d.name, if d.is_default { " (default)" } else { "" });
    }

    if network {
        let rt = tokio::runtime::Builder::new_current_thread().enable_all().build().expect("runtime");
        r.check(
            "Discord API reachable",
            rt.block_on(async {
                let start = Instant::now();
                dn_api::RestClient::gateway_url_unauthenticated(None)
                    .await
                    .map(|u| format!("{u} in {:?}", start.elapsed()))
                    .map_err(|e| e.to_string())
            }),
        );
        r.check("Discord gateway HELLO", rt.block_on(gateway_hello()));
    }

    if r.failed {
        println!("Self-check found problems.");
        ExitCode::FAILURE
    } else {
        println!("All checks passed.");
        ExitCode::SUCCESS
    }
}

/// Connects without a valid token: success means we received HELLO and Discord then
/// rejected the placeholder token (close 4004), proving the full gateway path works.
async fn gateway_hello() -> Result<String, String> {
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
    let cfg = dn_gateway::GatewayConfig::new("self-check.invalid.token", dn_gateway::Intents::GUILDS);
    let start = Instant::now();
    let handle = dn_gateway::spawn(cfg, tx);
    let outcome = tokio::time::timeout(Duration::from_secs(15), async {
        while let Some(ev) = rx.recv().await {
            if let dn_gateway::GatewayEvent::Fatal(f) = ev {
                return Some(f);
            }
        }
        None
    })
    .await;
    handle.shutdown().await;
    match outcome {
        Ok(Some(dn_gateway::FatalError::AuthenticationFailed)) => {
            Ok(format!("connected; placeholder token rejected as expected ({:?})", start.elapsed()))
        }
        Ok(Some(other)) => Err(format!("unexpected gateway error: {other}")),
        Ok(None) => Err("gateway closed without a response".into()),
        Err(_) => Err("timed out".into()),
    }
}
