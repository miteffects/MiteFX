//! Native file dialogs, without blocking the editor's frames: on Linux and Windows the dialog runs
//! on a worker thread and its answer arrives on a channel. macOS needs its panels on the main
//! thread, where the editor already runs, so there it runs in place (modal, as macOS expects).

use std::path::PathBuf;
use std::sync::mpsc::{channel, Receiver};

/// Asks for one file to open, offering `filters` (a name and its extensions). The answer is
/// `None` if the dialog was cancelled.
pub fn pick_file(title: &str, filters: &[(&str, &[&str])]) -> Receiver<Option<PathBuf>> {
    let mut dialog = rfd::FileDialog::new().set_title(title);
    for (name, extensions) in filters {
        dialog = dialog.add_filter(*name, extensions);
    }
    let (tx, rx) = channel();
    if cfg!(target_os = "macos") {
        let _ = tx.send(dialog.pick_file());
    } else {
        std::thread::spawn(move || {
            let _ = tx.send(dialog.pick_file());
        });
    }
    rx
}
