use crate::{db::Database, local, planner, remote, *};

/// Caller owns the profile lock. Pending work retains its original expectations;
/// the executor must revalidate them rather than silently changing a queued write.
pub async fn prepare(
    db: &mut Database,
    pair: &PairConfig,
    provider: &dyn Provider,
) -> Result<Plan> {
    let started = std::time::Instant::now();
    let mut entry = crate::activity::Entry::new(
        "scan",
        "started",
        "Rechecking local files and Drive changes",
    );
    entry.pair_id = Some(pair.id.clone());
    db.record_activity(&entry)?;
    let result = prepare_inner(db, pair, provider).await;
    entry.at_ms = crate::activity::Entry::new("scan", "completed", "").at_ms;
    entry.details.duration_ms = Some(started.elapsed().as_millis() as u64);
    match &result {
        Ok(plan) => {
            entry.outcome = "completed".into();
            entry.details.changes = Some(plan.operations.len());
            entry.details.conflicts = Some(plan.conflicts.len());
            entry.details.skipped = Some(plan.skipped.len());
            entry.message = format!(
                "Comparison complete: {} changes, {} conflicts, {} skipped items",
                plan.operations.len(),
                plan.conflicts.len(),
                plan.skipped.len()
            );
        }
        Err(error) => {
            entry.outcome = "error".into();
            entry.details.error_code = Some(error.code);
            entry.message = error.message.clone();
        }
    }
    db.record_activity(&entry)?;
    result
}
async fn prepare_inner(
    db: &mut Database,
    pair: &PairConfig,
    provider: &dyn Provider,
) -> Result<Plan> {
    let local = local::scan_pair(pair)?;
    if local.root_identity != pair.root_identity {
        return Err(Error::new(
            ErrorCode::IncompleteScan,
            "The local root has been replaced. Sync is stopped until the folder is reviewed.",
        ));
    }
    let mut pending = Vec::new();
    for op in db.operations(&pair.id)? {
        if crate::executor::ignore_guard(pair, &op)? {
            db.skip_operation(&op)?;
        } else {
            pending.push(op);
        }
    }
    if !pending.is_empty() {
        return Ok(Plan {
            operations: pending,
            conflicts: db.conflicts(&pair.id)?,
            ..Default::default()
        });
    }
    let base = db.baselines(&pair.id)?;
    let previous = db.get::<RemoteInventory>(&format!("remote:{}", pair.id))?;
    let remote = match remote::snapshot_from_cursor(
        provider,
        &pair.remote_root_id,
        &pair.excludes,
        &base,
        previous.as_ref().and_then(|r| r.cursor.as_deref()),
    )
    .await
    {
        Ok(remote) => remote,
        Err(error) => {
            db.set(&format!("offline_local:{}", pair.id), &local)?;
            return Err(error);
        }
    };
    let mut plan = planner::plan(pair, &local, &remote, &base)?;
    // Explicit conflicts remain visible until the user resolves them. Other
    // paths can keep syncing without treating a fresh scan as implicit consent.
    for existing in db.conflicts(&pair.id)? {
        plan.operations
            .retain(|o| !planner::beneath(&o.path, &existing.path));
        plan.accepted
            .retain(|b| !planner::beneath(&b.path, &existing.path));
        plan.forgotten
            .retain(|p| !planner::beneath(p, &existing.path));
        plan.conflicts.retain(|c| c.path != existing.path);
        plan.conflicts.push(existing);
    }
    db.persist_plan(&pair.id, &plan, &local, &remote)?;
    Ok(plan)
}
