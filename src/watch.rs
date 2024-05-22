use std::{fmt::Display, path::PathBuf, sync::Mutex};

use tokio::sync::mpsc::{self, Receiver};

use notify::{
    event::{DataChange, ModifyKind, RenameMode},
    Event, EventKind, RecommendedWatcher, RecursiveMode, Watcher,
};

use crate::{
    app_state::AppState,
    config::ServerConfiguration,
    library::{is_format_supported, MediaFolders, MediaType},
    utils,
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

fn watch_changes(
    paths: Vec<&PathBuf>,
) -> anyhow::Result<(RecommendedWatcher, Receiver<FileEvent>)> {
    let (tx, rx) = mpsc::channel(100);
    let mut watcher = notify::recommended_watcher(move |res: Result<Event, notify::Error>| {
        if let Ok(event) = res {
            tracing::debug!("folders change detected: {:?}", event);
            let event_type = match event.kind {
                EventKind::Create(_) => EventType::Create,
                EventKind::Modify(kind) if kind == ModifyKind::Name(RenameMode::To) => {
                    EventType::Create
                }
                EventKind::Modify(kind) if kind == ModifyKind::Data(DataChange::Content) => {
                    EventType::Modify
                }
                EventKind::Remove(_) => EventType::Remove,
                _ => {
                    return;
                }
            };
            tx.blocking_send(FileEvent {
                event_type,
                path: event.paths.first().unwrap().clone(),
            })
            .unwrap();
        }
    })?;

    for path in paths {
        watcher.watch(path, RecursiveMode::Recursive)?;
    }

    Ok((watcher, rx))
}

fn flatten_path(path: &PathBuf) -> Result<Vec<PathBuf>, anyhow::Error> {
    let mut flattened_paths = Vec::new();
    let metadata = path.metadata()?;
    if metadata.is_dir() {
        flattened_paths.append(&mut utils::walk_recursive(
            &path,
            Some(is_format_supported),
        )?);
    }
    if metadata.is_file() {
        flattened_paths.push(path.clone())
    }
    Ok(flattened_paths)
}

/// Monitor library changes (blocking)
pub async fn monitor_library(app_state: AppState, folders: MediaFolders) {
    let (_watcher, mut rx) = watch_changes(folders.all()).unwrap();
    while let Some(event) = rx.recv().await {
        if let Some(media_type) = folders.folder_type(&event.path) {
            match event.event_type {
                EventType::Create => {
                    let _ = match media_type {
                        MediaType::Show => {
                            let files_paths = flatten_path(&event.path).unwrap();
                            for path in files_paths {
                                tracing::info!("Detected new show file: {}", path.display())
                            }
                        }
                        MediaType::Movie => {
                            let files_paths = flatten_path(&event.path).unwrap();
                            for path in files_paths {
                                tracing::info!("Detected new movie file: {}", path.display())
                            }
                        }
                    };
                }
                EventType::Remove => {
                    _ = app_state.reconciliate_library().await;
                }
                EventType::Modify => {}
            };
        }
    }
}

pub async fn monitor_config(
    configuration: &'static Mutex<ServerConfiguration>,
    config_path: PathBuf,
) {
    let (mut watcher, mut rx) = watch_changes(vec![&config_path]).unwrap();
    while let Some(event) = rx.recv().await {
        match event.event_type {
            EventType::Modify => {
                let mut config = configuration.lock().unwrap();
                if let Ok(new_config) = config.config_file.read() {
                    tracing::info!("Detected config file changes");
                    config.apply_config_schema(new_config)
                }
            }
            EventType::Remove => {
                watcher
                    .watch(&config_path, RecursiveMode::NonRecursive)
                    .unwrap();
                let mut config = configuration.lock().unwrap();
                if let Ok(new_config) = config.config_file.read() {
                    tracing::info!("Detected config file changes");
                    config.apply_config_schema(new_config)
                }
            }
            _ => {}
        }
    }
}
