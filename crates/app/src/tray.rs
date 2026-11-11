//! Windows notification-area (tray) icon with Open / Quit.

use tray_icon::menu::{Menu, MenuEvent, MenuItem, PredefinedMenuItem};
use tray_icon::{Icon, MouseButton, MouseButtonState, TrayIcon, TrayIconBuilder, TrayIconEvent};

use crate::{ICON_RGBA, ShellEvent};

pub struct Tray {
    _icon: TrayIcon,
}

pub fn create(ctx: &egui::Context, tx: crossbeam_channel::Sender<ShellEvent>) -> Option<Tray> {
    let menu = Menu::new();
    let open = MenuItem::new("Open Discord Native", true, None);
    let quit = MenuItem::new("Quit", true, None);
    menu.append(&open).ok()?;
    menu.append(&PredefinedMenuItem::separator()).ok()?;
    menu.append(&quit).ok()?;
    let icon = Icon::from_rgba(ICON_RGBA.to_vec(), 64, 64).ok()?;
    let tray = match TrayIconBuilder::new().with_menu(Box::new(menu)).with_tooltip("Discord Native").with_icon(icon).with_menu_on_left_click(false).build() {
        Ok(t) => t,
        Err(e) => {
            tracing::warn!("tray icon unavailable: {e}");
            return None;
        }
    };
    let (open_id, quit_id) = (open.id().clone(), quit.id().clone());
    {
        let (tx, ctx) = (tx.clone(), ctx.clone());
        MenuEvent::set_event_handler(Some(move |e: MenuEvent| {
            let ev = if e.id == open_id {
                ShellEvent::Show
            } else if e.id == quit_id {
                ShellEvent::Quit
            } else {
                return;
            };
            let _ = tx.send(ev);
            // Wakes eframe even while the window is hidden so App::logic runs.
            ctx.request_repaint();
        }));
    }
    {
        let ctx = ctx.clone();
        TrayIconEvent::set_event_handler(Some(move |e: TrayIconEvent| {
            if let TrayIconEvent::Click { button: MouseButton::Left, button_state: MouseButtonState::Up, .. } = e {
                let _ = tx.send(ShellEvent::Show);
                ctx.request_repaint();
            }
        }));
    }
    Some(Tray { _icon: tray })
}
