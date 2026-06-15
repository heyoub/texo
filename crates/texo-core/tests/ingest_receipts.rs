//! PROVES: INV-RECEIPT-VERIFIED

mod support;

use support::{copy_sample_sources, ingest_sample_sources, setup_demo_journal, temp_workspace};
use texo_core::{open_journal, verify_journal_receipts};

#[test]
fn ingest_receipts_verify_against_store() {
    let dir = temp_workspace();
    copy_sample_sources(dir.path());
    setup_demo_journal(dir.path());
    ingest_sample_sources(dir.path());

    let journal = open_journal(dir.path()).expect("open");
    let workspace = journal.config().workspace().expect("workspace");
    verify_journal_receipts(journal.handle().store(), &workspace).expect("journal receipts");
    journal.close().expect("close");
}
