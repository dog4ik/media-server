use std::{collections::HashMap, path::Path};

use tokio::sync::mpsc;
use tray_icon::{
    menu::{IsMenuItem, Menu, MenuEvent, MenuId, MenuItem, PredefinedMenuItem},
    TrayIconBuilder, TrayIconEvent,
};
use winit::{
    event_loop::{ControlFlow, EventLoopBuilder},
    platform::windows::EventLoopBuilderExtWindows,
};

use crate::app_state::AppState;

#[derive(Debug, Clone, Copy)]
pub enum ButtonType {
    IconDoubleClick,
    IconLeftClick,
    RefreshLibrary,
    Open,
    Exit,
}

pub struct ButtonRegistry {
    pub registry: HashMap<MenuId, ButtonType>,
    pub menu: Menu,
}

/// Important to setup registry BEFORE moving it in event loop
impl ButtonRegistry {
    pub fn new() -> Self {
        Self {
            registry: HashMap::new(),
            menu: Menu::new(),
        }
    }
    pub fn register(&mut self, item: &dyn IsMenuItem, btn_type: ButtonType) {
        self.registry.insert(item.id().clone(), btn_type);
        self.menu.append(item).unwrap();
    }
    pub fn spererator(&self) {
        self.menu.append(&PredefinedMenuItem::separator()).unwrap();
    }

    pub fn get_element(&self, id: &MenuId) -> Option<&ButtonType> {
        self.registry.get(id)
    }
}

#[cfg(target_os = "windows")]
fn windows_tray_icon(sender: mpsc::Sender<ButtonType>) {
    use std::time::Duration;

    let event_loop = EventLoopBuilder::new()
        .with_any_thread(true)
        .build()
        .unwrap();

    #[cfg(target_os = "windows")]
    let mut tray_icon = None;

    let menu_channel = MenuEvent::receiver();
    let tray_channel = TrayIconEvent::receiver();
    let registry = menu();

    event_loop
        .run(move |event, event_loop| {
            event_loop.set_control_flow(ControlFlow::Wait);

            if let winit::event::Event::NewEvents(winit::event::StartCause::Init) = event {
                let icon = load_icon("dist/logo.webp");
                tray_icon = Some(
                    TrayIconBuilder::new()
                        .with_menu(Box::new(registry.menu.clone()))
                        .with_tooltip("Media server")
                        .with_icon(icon)
                        .with_title("Media server")
                        .build()
                        .unwrap(),
                );
            }

            if let Ok(event) = tray_channel.try_recv() {
                // NOTE: this dont work as expected when control flow set to Wait
                match event.click_type {
                    tray_icon::ClickType::Right => (),
                    tray_icon::ClickType::Left => {
                        sender.blocking_send(ButtonType::IconLeftClick).unwrap();
                    }
                    tray_icon::ClickType::Double => {
                        sender.blocking_send(ButtonType::IconDoubleClick).unwrap();
                    }
                }
            }
            if let Ok(event) = menu_channel.try_recv() {
                if let Some(btn) = registry.get_element(event.id()) {
                    sender.blocking_send(btn.clone()).unwrap();
                }
            }
        })
        .unwrap();
}

pub async fn spawn_tray_icon(app_state: AppState) {
    let (tx, mut rx) = mpsc::channel(10);
    #[cfg(target_os = "windows")]
    std::thread::spawn(move || windows_tray_icon(tx.clone()));

    while let Some(pressed_btn) = rx.recv().await {
        match pressed_btn {
            ButtonType::RefreshLibrary => {
                tracing::trace!("Exit tray button pressed");
                app_state.reconciliate_library().await.unwrap();
            }
            ButtonType::Exit => {
                app_state.cancelation_token.cancel();
                tracing::trace!("Exit tray button pressed");
            }
            ButtonType::IconDoubleClick => {
                tracing::trace!("Icon doubleclicked");
            }
            ButtonType::IconLeftClick => {
                tracing::trace!("Icon leftclicked");
            }
            ButtonType::Open => {
                tracing::trace!("Open tray button pressed");
                open::that("http://localhost:6969").unwrap();
            }
        }
    }
}

fn menu() -> ButtonRegistry {
    let mut registry = ButtonRegistry::new();
    let open = MenuItem::new("Open", true, None);
    let refresh = MenuItem::new("Refresh Library", true, None);
    let exit = MenuItem::new("Exit", true, None);
    registry.register(&open, ButtonType::Open);
    registry.register(&refresh, ButtonType::RefreshLibrary);
    registry.spererator();
    registry.register(&exit, ButtonType::Exit);
    registry
}

pub fn load_icon(path: impl AsRef<Path>) -> tray_icon::Icon {
    let (icon_rgba, icon_width, icon_height) = {
        let image = image::open(path)
            .expect("Failed to open icon path")
            .into_rgba8();
        let (width, height) = image.dimensions();
        let rgba = image.into_raw();
        (rgba, width, height)
    };
    tray_icon::Icon::from_rgba(icon_rgba, icon_width, icon_height).expect("Failed to open icon")
}
