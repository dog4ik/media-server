use std::{fmt::Display, path::PathBuf};

use tokio::sync::mpsc::{self};

use notify::{Event, EventKind, RecursiveMode, Watcher};

use crate::{
    app_state::AppState,
    config::{self, APP_RESOURCES},
};

#[derive(Debug, Clone)]
enum EventType {
    Create,
    Remove,
    Modify,
}

#[derive(Debug, Clone)]
struct FileEvent {
    event_type: EventType,
    path: PathBuf,
}

impl Display for FileEvent {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self.event_type {
            EventType::Create => write!(f, "creation"),
            EventType::Remove => write!(f, "remove"),
            EventType::Modify => write!(f, "modification"),
        }?;
        f.write_str(&self.path.to_string_lossy())
    }
}

#[derive(Debug, Clone)]
enum WatchCommand {
    Watch(PathBuf),
    UnWatch(PathBuf),
}

#[derive(Debug)]
struct FileWatcher {
    tx: mpsc::Sender<WatchCommand>,
}

impl FileWatcher {
    pub fn new(app_state: AppState) -> anyhow::Result<Self> {
        let (notify_tx, mut notify_rx) = mpsc::channel(100);
        let (command_tx, mut command_rx) = mpsc::channel(100);
        let mut watcher = notify::recommended_watcher(move |res: Result<Event, notify::Error>| {
            let _ = notify_tx.blocking_send(res);
        })?;

        let cancellation_token = app_state.cancelation_token.clone();
        let mut show_dirs: config::ShowFolders = config::CONFIG.get_value();
        let mut movie_dirs: config::MovieFolders = config::CONFIG.get_value();
        let config_path = APP_RESOURCES.get().unwrap().config_path.clone();

        tokio::spawn(async move {
            loop {
                tokio::select! {
                    Some(event) = notify_rx.recv() => {
                        match event {
                            Ok(event) => match event.kind {
                                EventKind::Remove(_)
                                | EventKind::Create(_)
                                | EventKind::Modify(_) => {
                                    tracing::debug!("Received watcher event: {:?}", event);
                                    for path in event.paths {
                                        if path == config_path {
                                            // load config
                                        }
                                        if show_dirs.0.contains(&path) {
                                            app_state.partial_refresh().await;
                                        }
                                        if movie_dirs.0.contains(&path) {
                                            app_state.partial_refresh().await;
                                        }
                                    }
                                }
                                _ => (),
                            },
                            Err(err) => {
                                tracing::debug!("Config watcher errors: {:?}", err);
                            }
                        };
                    }
                    Some(command) = command_rx.recv() => {
                        match command {
                            WatchCommand::Watch(path) => {
                                if let Err(e) = watcher.watch(&path, RecursiveMode::NonRecursive) {
                                    tracing::error!("Failed to add {} to the watcher: {e}", path.display());
                                } else {
                                    show_dirs.add(path);
                                };
                            },
                            WatchCommand::UnWatch(path) => {
                                let _ = watcher.unwatch(&path);
                                show_dirs.0.retain(|p| *p != path);
                                movie_dirs.0.retain(|p| *p != path);
                            },
                        }
                    }
                    _ = cancellation_token.cancelled() => break,
                }
            }
        });

        Ok(Self { tx: command_tx })
    }

    pub fn watch(&self, path: PathBuf) {
        self.tx.try_send(WatchCommand::Watch(path)).unwrap();
    }
    pub fn unwatch(&self, path: PathBuf) {
        self.tx.try_send(WatchCommand::UnWatch(path)).unwrap();
    }
}
