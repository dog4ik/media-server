use std::{fmt::Display, path::PathBuf};

use tokio::{
    sync::mpsc::{self, Receiver},
    task::JoinHandle,
};

use notify::{
    event::{ModifyKind, RenameMode},
    Event, EventKind, RecommendedWatcher, RecursiveMode, Watcher,
};

use crate::{
    app_state::AppState,
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
            EventType::Create => write!(f, "detected file creation: "),
            EventType::Remove => write!(f, "detected file remove: "),
            EventType::Modify => write!(f, "detected file modification: "),
        }?;
        f.write_str(&self.path.to_string_lossy())
    }
}

fn watch_changes(
    folders: Vec<&PathBuf>,
) -> anyhow::Result<(RecommendedWatcher, Receiver<FileEvent>)> {
    let (tx, rx) = mpsc::channel(100);
    let mut watcher = notify::recommended_watcher(move |res: Result<Event, notify::Error>| {
        if let Ok(event) = res {
            tracing::debug!("folders change detected");
            let event_type = match event.kind {
                EventKind::Create(_) => EventType::Create,
                EventKind::Modify(kind) if kind == ModifyKind::Name(RenameMode::To) => {
                    EventType::Create
                }
                EventKind::Remove(_) => EventType::Remove,
                EventKind::Modify(kind) if kind == ModifyKind::Name(RenameMode::From) => {
                    EventType::Remove
                }
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

    for folder in folders {
        watcher.watch(folder, RecursiveMode::Recursive)?;
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

pub async fn monitor_library(app_state: AppState, folders: MediaFolders) -> JoinHandle<()> {
    let metadata_provider = app_state.tmdb_api;
    tokio::spawn(async move {
        let (_watcher, mut rx) = watch_changes(folders.all()).unwrap();
        while let Some(event) = rx.recv().await {
            if let Some(media_type) = folders.folder_type(&event.path) {
                match event.event_type {
                    EventType::Create => {
                        let _ = match media_type {
                            MediaType::Show => {
                                let files_paths = flatten_path(&event.path).unwrap();
                                for path in files_paths {
                                    let _ = app_state.add_show(path, metadata_provider).await;
                                }
                            }
                            MediaType::Movie => {
                                let files_paths = flatten_path(&event.path).unwrap();
                                for path in files_paths {
                                    let _ = app_state.add_movie(path, metadata_provider).await;
                                }
                            }
                        };
                    }
                    EventType::Remove => {
                        _ = app_state.reconciliate_library().await;
                    }
                    EventType::Modify => {
                        let _ = app_state.reconciliate_library().await;
                    }
                };
            }
        }
    })
}
