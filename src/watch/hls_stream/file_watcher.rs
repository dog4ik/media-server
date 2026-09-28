use std::path::{Path, PathBuf};

use notify::{
    Event, EventKind, RecommendedWatcher, RecursiveMode, Watcher,
    event::{AccessKind, AccessMode, ModifyKind, RenameMode},
};
use tokio::sync::mpsc::Receiver;

pub fn spawn_watcher(
    path: impl AsRef<Path>,
) -> notify::Result<(RecommendedWatcher, Receiver<PathBuf>)> {
    let (tx, rx) = tokio::sync::mpsc::channel(1);
    let mut watcher = RecommendedWatcher::new(
        move |res| match res {
            Ok(Event {
                kind:
                    EventKind::Access(AccessKind::Close(AccessMode::Write))
                    | EventKind::Modify(ModifyKind::Name(RenameMode::To)),
                paths,
                ..
            }) => {
                let path = &paths[0];
                if path.extension().is_some_and(|ext| ext == "tmp") {
                    return;
                }
                tracing::trace!("Detected finished file: {}", path.display());
                tx.blocking_send(path.clone()).unwrap();
            }
            Ok(_) => {}
            Err(_) => {}
        },
        Default::default(),
    )?;

    watcher.watch(path.as_ref(), RecursiveMode::NonRecursive)?;

    Ok((watcher, rx))
}
