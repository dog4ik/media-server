use std::{collections::HashMap, path::Path};

use tokio::sync::mpsc;
use tray_icon::{
    TrayIcon, TrayIconBuilder, TrayIconEvent,
    menu::{IsMenuItem, Menu, MenuEvent, MenuId, MenuItem, PredefinedMenuItem},
};
use winit::{
    application::ApplicationHandler,
    event::WindowEvent,
    event_loop::{ActiveEventLoop, ControlFlow, EventLoop},
    platform::windows::EventLoopBuilderExtWindows,
    window::WindowId,
};

use crate::config::APP_RESOURCES;
use crate::{app_state::AppState, config};

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

impl Default for ButtonRegistry {
    fn default() -> Self {
        Self {
            registry: HashMap::new(),
            menu: Menu::new(),
        }
    }
}

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

    pub fn button_type(&self, id: &MenuId) -> Option<&ButtonType> {
        self.registry.get(id)
    }
}

struct Tray {
    tray_icon: Option<TrayIcon>,
    tx: mpsc::Sender<ButtonType>,
}

impl Tray {
    pub fn new(sender: mpsc::Sender<ButtonType>) -> Self {
        Self {
            tx: sender,
            tray_icon: None,
        }
    }
}

impl ApplicationHandler for Tray {
    fn resumed(&mut self, _event_loop: &ActiveEventLoop) {
        if self.tray_icon.is_some() {
            return;
        }
        let registry = menu();
        let mut builder = TrayIconBuilder::new()
            .with_menu(Box::new(registry.menu))
            .with_menu_on_left_click(false)
            .with_title("Media server");
        let base_path = APP_RESOURCES.statics_path.clone();
        if let Ok(icon) = load_icon(base_path.join("dist/logo.webp")) {
            builder = builder.with_icon(icon);
        }
        let tray = builder.build().unwrap();
        {
            let tx = self.tx.clone();
            MenuEvent::set_event_handler(Some(move |menu_event: MenuEvent| {
                let element = *registry
                    .registry
                    .get(&menu_event.id())
                    .expect("All elements are registered");
                tx.blocking_send(element).unwrap();
            }));
        }
        {
            let tx = self.tx.clone();
            TrayIconEvent::set_event_handler(Some(move |tray_event| {
                match tray_event {
                    TrayIconEvent::DoubleClick { .. } => {
                        tx.blocking_send(ButtonType::Open).unwrap();
                    }
                    _ => {}
                };
            }));
        }
        self.tray_icon = Some(tray);
    }

    fn window_event(
        &mut self,
        _event_loop: &ActiveEventLoop,
        _window_id: WindowId,
        _event: WindowEvent,
    ) {
    }
}

#[cfg(target_os = "windows")]
fn windows_tray_icon(sender: mpsc::Sender<ButtonType>) {
    let event_loop = EventLoop::builder().with_any_thread(true).build().unwrap();
    event_loop.set_control_flow(ControlFlow::Wait);

    let mut tray = Tray::new(sender);
    event_loop.run_app(&mut tray).unwrap();
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
                tracing::trace!("Exit tray button pressed");
                app_state.cancelation_token.cancel();
            }
            ButtonType::IconDoubleClick => {
                tracing::trace!("Icon doubleclicked");
            }
            ButtonType::IconLeftClick => {
                tracing::trace!("Icon leftclicked");
            }
            ButtonType::Open => {
                tracing::trace!("Open tray button pressed");
                let port: config::Port = config::CONFIG.get_value();
                open::that(format!("http://127.0.0.1:{}", port.0)).unwrap();
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

pub fn load_icon(path: impl AsRef<Path>) -> anyhow::Result<tray_icon::Icon> {
    let (icon_rgba, icon_width, icon_height) = {
        let image = image::open(path)?.into_rgba8();
        let (width, height) = image.dimensions();
        let rgba = image.into_raw();
        (rgba, width, height)
    };
    let icon = tray_icon::Icon::from_rgba(icon_rgba, icon_width, icon_height)?;
    Ok(icon)
}
