//! Exhaustive CLI command routing.

use std::process::ExitCode;

use crate::error::TexoError;

use super::{Command, DispatchContext};

mod durability;
mod integrations;
mod semantics;
mod surfaces;
mod workspace;

pub(super) fn route(cli: &DispatchContext, command: Command) -> Result<ExitCode, TexoError> {
    match command {
        Command::Init { workspace } => workspace::init(cli, &workspace),
        Command::Ingest {
            path,
            dry_run,
            strict,
            json,
        } => workspace::ingest(cli, &path, dry_run, strict, json),
        Command::Claims { subject, json } => workspace::claims(cli, subject.as_deref(), json),
        Command::Supersede {
            old,
            new,
            reason,
            decided_by,
            json,
        } => workspace::supersede(cli, &old, &new, &reason, &decided_by, json),
        Command::CheckStaleness { path, json } => workspace::check_staleness(cli, &path, json),
        Command::AgentContext {
            subject,
            out,
            json,
            allow_unsettled,
        } => workspace::agent_context(cli, subject.as_deref(), out, json, allow_unsettled),
        Command::Compile {
            out,
            allow_unsettled,
        } => workspace::compile(cli, &out, allow_unsettled),
        Command::Relate {
            json,
            strict,
            pair_budget,
            candidate_cursor,
            rejudge_pair,
        } => semantics::relate(
            cli,
            json,
            strict,
            pair_budget,
            candidate_cursor,
            rejudge_pair.as_deref(),
        ),
        Command::Conflicts { json, commit } => workspace::conflicts(cli, json, commit),
        Command::Verify { json } => workspace::verify(cli, json),
        Command::Stats { json } => workspace::stats(cli, json),
        Command::Index {
            scip,
            max_files,
            max_file_bytes,
            max_total_bytes,
            json,
        } => semantics::index(
            cli,
            scip.as_deref(),
            max_files,
            max_file_bytes,
            max_total_bytes,
            json,
        ),
        Command::Reconcile {
            max_per_claim,
            max_candidates,
            min_score_ppm,
            json,
        } => semantics::reconcile(cli, max_per_claim, max_candidates, min_score_ppm, json),
        Command::Mcp => surfaces::mcp(cli),
        Command::Serve(options) => {
            surfaces::serve(cli, options.with_default_journal(cli.journal.as_deref()))
        }
        Command::Extract { path } => Ok(surfaces::extract(&path)),
        Command::Session { cmd } => surfaces::session(cli, cmd),
        Command::Host { cmd } => surfaces::host(cli, &cmd),
        Command::Ops { cmd } => surfaces::ops(cmd),
        Command::Install {
            client,
            dry_run,
            json,
        } => integrations::install(cli, &client, dry_run, json),
        Command::Uninstall {
            client,
            dry_run,
            json,
        } => integrations::uninstall(cli, &client, dry_run, json),
        Command::Hook { cmd } => integrations::hook(cli, &cmd),
        Command::Doctor { deep, fix, json } => integrations::doctor(cli, deep, fix, json),
        Command::Backup { cmd } => durability::backup(cli, cmd),
        Command::Replica { cmd } => durability::replica(cli, cmd),
    }
}
