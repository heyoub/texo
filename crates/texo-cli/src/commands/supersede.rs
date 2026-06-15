//! supersede command.

use anyhow::Result;
use texo_core::{supersede_claim, ClaimId};

use crate::observed_at_ms;

pub fn run(
    root: &std::path::Path,
    old: &str,
    new: &str,
    reason: &str,
    decided_by: &str,
    json: bool,
) -> Result<()> {
    let old_id = ClaimId::try_from(old).map_err(|e| anyhow::anyhow!("{e}"))?;
    let new_id = ClaimId::try_from(new).map_err(|e| anyhow::anyhow!("{e}"))?;
    let receipt = supersede_claim(root, &old_id, &new_id, reason, decided_by, observed_at_ms())?;
    if json {
        println!("{}", serde_json::to_string_pretty(&receipt)?);
    } else {
        println!(
            "superseded {old} with {new} at local seq {}",
            receipt.sequence.get()
        );
    }
    Ok(())
}
