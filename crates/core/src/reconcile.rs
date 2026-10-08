use crate::{db::Database, local, planner, remote, *};

/// Caller owns the profile lock. Pending work retains its original expectations;
/// the executor must revalidate them rather than silently changing a queued write.
pub async fn prepare(
    db: &mut Database,
    pair: &PairConfig,
    provider: &dyn Provider,
) -> Result<Plan> {
    let local = local::scan(&pair.local_root, &pair.excludes)?;
    if local.root_identity != pair.root_identity {
        return Err(Error::new(
            ErrorCode::IncompleteScan,
            "The local root has been replaced. Sync is stopped until the folder is reviewed.",
        ));
    }
    let pending = db.operations(&pair.id)?;
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
