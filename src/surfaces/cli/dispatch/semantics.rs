//! Code-intelligence and semantic-settlement commands.

use std::path::Path;
use std::process::ExitCode;

use serde_json::{json, Value};

use crate::error::TexoError;

use super::super::{observed_at_ms, open_host, render, DispatchContext};

pub(super) fn index(
    cli: &DispatchContext,
    scip: Option<&Path>,
    max_files: Option<usize>,
    max_file_bytes: Option<u64>,
    max_total_bytes: Option<u64>,
    json_output: bool,
) -> Result<ExitCode, TexoError> {
    let _ = json_output;
    let mut host = open_host(&cli.root, cli.workspace.as_deref(), cli.journal.as_deref())?;
    let observed_at_ms = observed_at_ms();
    let source = host.invoke_json(
        "texo.knowledge.index",
        &json!({
            "observed_at_ms": observed_at_ms,
            "max_files": max_files,
            "max_file_bytes": max_file_bytes,
            "max_total_bytes": max_total_bytes
        }),
    )?;
    let code = host.invoke_json(
        "texo.code.index.build",
        &json!({
            "snapshot_id": source.get("snapshot_id").cloned(),
            "scip_path": scip,
            "observed_at_ms": observed_at_ms
        }),
    )?;
    let output = json!({
        "schema": "texo.index.v2",
        "source": source,
        "code": code
    });
    let truncated = ["source", "code"].iter().any(|phase| {
        output
            .get(*phase)
            .and_then(|value| value.get("coverage"))
            .and_then(|coverage| coverage.get("truncated"))
            .and_then(Value::as_bool)
            .unwrap_or(false)
    });
    render::json(&output)?;
    Ok(if truncated {
        ExitCode::from(2)
    } else {
        ExitCode::SUCCESS
    })
}

pub(super) fn reconcile(
    cli: &DispatchContext,
    max_per_claim: Option<usize>,
    max_candidates: Option<usize>,
    min_score_ppm: Option<u32>,
    json_output: bool,
) -> Result<ExitCode, TexoError> {
    let mut host = open_host(&cli.root, cli.workspace.as_deref(), cli.journal.as_deref())?;
    let output = host.invoke_json(
        "texo.knowledge.reconcile",
        &json!({
            "observed_at_ms": observed_at_ms(),
            "max_per_claim": max_per_claim,
            "max_candidates": max_candidates,
            "min_score_ppm": min_score_ppm,
            "budget_secs": null,
            "concurrency": null
        }),
    )?;
    if json_output {
        render::json(&output)?;
    } else {
        render::reconcile(&output)?;
    }
    Ok(partial_exit(&output))
}

pub(super) fn relate(
    cli: &DispatchContext,
    json_output: bool,
    strict: bool,
    pair_budget: Option<usize>,
    candidate_cursor: Option<u64>,
    rejudge_pair: Option<&[String]>,
) -> Result<ExitCode, TexoError> {
    let mut host = open_host(&cli.root, cli.workspace.as_deref(), cli.journal.as_deref())?;
    let output = host.invoke_json(
        "texo.relate.run",
        &json!({
            "observed_at_ms": observed_at_ms(),
            "strict": strict,
            "max_candidate_pairs": pair_budget,
            "candidate_cursor": candidate_cursor,
            "rejudge_pair": rejudge_pair
        }),
    )?;
    if json_output {
        render::json(&output)?;
    } else {
        render::relate(&output)?;
    }
    Ok(partial_exit(&output))
}

fn partial_exit(output: &Value) -> ExitCode {
    if output.get("outcome").and_then(Value::as_str) == Some("partial") {
        ExitCode::from(2)
    } else {
        ExitCode::SUCCESS
    }
}
