//! Single-instance guard: the first instance listens on a loopback port derived from the
//! data directory; later launches ask it to show its window and exit.

use std::io::{BufRead, BufReader, Write};
use std::net::{Ipv4Addr, SocketAddr, TcpListener, TcpStream};
use std::time::Duration;

use dn_settings::Paths;

use crate::ShellEvent;

pub enum Instance {
    Primary(TcpListener),
    AlreadyRunning,
    /// Port taken by something else; run without single-instance support.
    Unavailable,
}

const MAGIC: &str = "discord-native:show";

pub fn port_for(paths: &Paths) -> u16 {
    let mut h: u32 = 0x811c_9dc5;
    for b in paths.data_dir.to_string_lossy().as_bytes() {
        h ^= *b as u32;
        h = h.wrapping_mul(0x0100_0193);
    }
    40_000 + (h % 20_000) as u16
}

pub fn acquire(paths: &Paths) -> Instance {
    let addr = SocketAddr::from((Ipv4Addr::LOCALHOST, port_for(paths)));
    match TcpListener::bind(addr) {
        Ok(l) => Instance::Primary(l),
        Err(_) => match TcpStream::connect_timeout(&addr, Duration::from_millis(500)) {
            Ok(mut s) => {
                let _ = s.set_read_timeout(Some(Duration::from_millis(500)));
                let _ = writeln!(s, "{MAGIC}");
                let mut reply = String::new();
                let _ = BufReader::new(&s).read_line(&mut reply);
                if reply.trim() == "ok" { Instance::AlreadyRunning } else { Instance::Unavailable }
            }
            Err(_) => Instance::Unavailable,
        },
    }
}

pub fn serve(listener: TcpListener, tx: crossbeam_channel::Sender<ShellEvent>, ctx: egui::Context) {
    let _ = std::thread::Builder::new().name("dn-instance".into()).spawn(move || {
        for stream in listener.incoming().flatten() {
            let _ = stream.set_read_timeout(Some(Duration::from_secs(1)));
            let mut line = String::new();
            let mut reader = BufReader::new(&stream);
            if reader.read_line(&mut line).is_ok() && line.trim() == MAGIC {
                let _ = (&stream).write_all(b"ok\n");
                if tx.send(ShellEvent::Show).is_err() {
                    break;
                }
                ctx.request_repaint();
            }
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn second_instance_signals_first() {
        let dir = tempfile::tempdir().unwrap();
        let paths = Paths::under(dir.path());
        let Instance::Primary(listener) = acquire(&paths) else {
            // Another process happens to own the port; nothing to verify.
            return;
        };
        let (tx, rx) = crossbeam_channel::unbounded();
        serve(listener, tx, egui::Context::default());
        assert!(matches!(acquire(&paths), Instance::AlreadyRunning));
        assert_eq!(rx.recv_timeout(Duration::from_secs(2)).unwrap(), ShellEvent::Show);
    }

    #[test]
    fn port_is_stable_and_in_range() {
        let p = Paths::under(std::path::Path::new("/tmp/x"));
        assert_eq!(port_for(&p), port_for(&p));
        assert!((40_000..60_000).contains(&port_for(&p)));
    }
}
