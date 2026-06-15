//! verify command.

use anyhow::Result;
use texo_core::{open_journal, verify_journal_receipts, verify_projection};

pub fn run(root: &std::path::Path, json: bool) -> Result<()> {
    let journal = open_journal(root)?;
    let workspace = journal.config().workspace()?;
    let replayed = journal.replay(&workspace)?;
    let projection = verify_projection(&replayed.state);
    let journal_ok = verify_journal_receipts(journal.handle().store(), &workspace);
    journal.close()?;

    if json {
        let projection_ok = projection.is_ok();
        let journal_ok_flag = journal_ok.is_ok();
        let mut errors = Vec::new();
        if let Err(err) = projection {
            errors.push(err.to_string());
        }
        if let Err(err) = journal_ok {
            errors.push(err.to_string());
        }
        let payload = serde_json::json!({
            "projection_ok": projection_ok,
            "journal_ok": journal_ok_flag,
            "replayed_through_sequence": replayed.state.replayed_through_sequence,
            "errors": errors,
        });
        println!("{}", serde_json::to_string_pretty(&payload)?);
        return Ok(());
    }

    if let Err(err) = projection {
        anyhow::bail!("verify failed (projection): {err}");
    }
    if let Err(err) = journal_ok {
        anyhow::bail!("verify failed (journal): {err}");
    }
    println!(
        "ok — replayed through local seq {}",
        replayed.state.replayed_through_sequence
    );
    Ok(())
}
