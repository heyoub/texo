//! Compile-fail tests for typestate guarantees.

use std::path::Path;

use texo_core::{init_workspace, open_journal, Journal, Closed};

fn cannot_use_handle_on_closed(root: &Path) {
    init_workspace(root, "demo").unwrap();
    let journal = open_journal(root).unwrap();
    let closed: Journal<Closed> = journal.close().unwrap();
    let _ = closed.handle(); //~ ERROR no method named `handle`
}

fn main() {}
