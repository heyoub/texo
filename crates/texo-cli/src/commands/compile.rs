//! compile command.

use std::path::Path;

use anyhow::Result;
use texo_core::compile_out;

use crate::observed_at_ms;

pub fn run(root: &Path, out: &Path) -> Result<()> {
    let output = compile_out(root, out, observed_at_ms())?;
    for (name, _) in &output.files {
        println!("wrote {}/{}", out.display(), name);
    }
    Ok(())
}
