use std::path::{Path, PathBuf};

use notify::{
    Event, EventKind, RecommendedWatcher, RecursiveMode, Watcher,
    event::{CreateKind, DataChange, ModifyKind},
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
                    EventKind::Create(CreateKind::File)
                    | EventKind::Modify(ModifyKind::Data(DataChange::Content)),
                paths,
                ..
            }) => {
                eprintln!("Detected file change: {}", paths[0].display());
                tx.blocking_send(paths[0].clone()).unwrap();
            }
            Ok(_) => {}
            Err(_) => {}
        },
        Default::default(),
    )?;

    watcher.watch(path.as_ref(), RecursiveMode::NonRecursive)?;

    Ok((watcher, rx))
}
