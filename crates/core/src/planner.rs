//! Three-way reconciliation. Inputs must be complete, validated inventories.
use crate::{local::Exclusions, *};
use std::collections::{BTreeMap, BTreeSet};

pub fn beneath(path: &str, prefix: &str) -> bool {
    path == prefix || path.starts_with(&format!("{prefix}/"))
}
fn conflict(
    pair: &PairConfig,
    path: &str,
    reason: &str,
    l: Option<&LocalEntry>,
    r: Option<&RemoteItem>,
) -> Conflict {
    Conflict {
        id: uuid::Uuid::new_v4().to_string(),
        pair_id: pair.id.clone(),
        path: path.into(),
        reason: reason.into(),
        local: l.cloned(),
        remote: r.cloned(),
    }
}
fn operation(
    pair: &PairConfig,
    path: &str,
    action: Action,
    l: Option<&LocalEntry>,
    r: Option<&RemoteItem>,
) -> Operation {
    Operation {
        id: uuid::Uuid::new_v4().to_string(),
        pair_id: pair.id.clone(),
        path: path.into(),
        action,
        expected_local: l.cloned(),
        expected_remote: r.cloned(),
        state: OperationState::Prepared,
        attempts: 0,
        retry_at: 0,
        upload_session: None,
        reserved_remote_id: None,
        error: None,
    }
}
fn accept(plan: &mut Plan, path: &str, l: &LocalEntry, r: &RemoteItem) {
    plan.accepted.push(Baseline {
        path: path.into(),
        local: l.clone(),
        remote: r.clone(),
    });
}
fn single(remote: &RemoteInventory, path: &str) -> Option<RemoteItem> {
    remote
        .entries
        .get(path)
        .filter(|v| v.len() == 1)
        .map(|v| v[0].clone())
}

pub fn plan(
    pair: &PairConfig,
    local: &LocalInventory,
    remote: &RemoteInventory,
    baselines: &[Baseline],
) -> Result<Plan> {
    if local.root_identity != pair.root_identity || remote.cursor.is_none() {
        return Err(Error::new(
            ErrorCode::IncompleteScan,
            "The selected root or inventory is unhealthy. No work was planned.",
        ));
    }
    let ignore = Exclusions::for_pair(pair)?;
    let mut result = Plan::default();
    let base: BTreeMap<_, _> = baselines.iter().map(|b| (b.path.clone(), b)).collect();
    let mut blocked: BTreeSet<String> = local.skipped.iter().map(|s| s.path.clone()).collect();
    result.skipped = local.skipped.clone();
    for (path, items) in &remote.entries {
        if ignore.excludes_kind(
            path,
            items.first().map(|r| r.kind).unwrap_or(ItemKind::File),
        )? {
            blocked.insert(path.clone());
            result.skipped.push(SkippedItem {
                path: path.clone(),
                reason: "Excluded by folder options or .gitignore".into(),
            });
            continue;
        }
        crate::local::validate_relative(path)?;
        if items.len() != 1 {
            blocked.insert(path.clone());
            result.conflicts.push(conflict(
                pair,
                path,
                "Multiple Drive items have this name. Select or rename them before syncing.",
                local.entries.get(path),
                None,
            ));
        } else if !matches!(items[0].kind, ItemKind::File | ItemKind::Folder) {
            blocked.insert(path.clone());
            result.skipped.push(SkippedItem {
                path: path.clone(),
                reason:
                    "Google documents and shortcuts need an explicit export/link policy (Phase 9)."
                        .into(),
            });
        }
    }
    let mut consumed = BTreeSet::new();
    // A stable file identity / Drive ID, plus unchanged content and descendants,
    // distinguishes a move from unrelated delete/create operations. Ambiguity is a conflict.
    for b in baselines {
        if ignore.excludes_kind(&b.path, b.local.kind)?
            || blocked.iter().any(|p| beneath(&b.path, p))
            || consumed.contains(&b.path)
        {
            continue;
        }
        let local_old = local.entries.get(&b.path);
        let remote_old = single(remote, &b.path);
        let local_moves: Vec<_> = local
            .entries
            .iter()
            .filter(|(p, l)| {
                **p != b.path
                    && !base.contains_key(*p)
                    && l.file_identity.is_some()
                    && l.file_identity == b.local.file_identity
            })
            .collect();
        let remote_moves: Vec<_> = remote
            .entries
            .iter()
            .filter(|(p, items)| **p != b.path && items.len() == 1 && items[0].id == b.remote.id)
            .collect();
        let move_candidate =
            if local_old.is_none() && local_moves.len() == 1 && remote_moves.is_empty() {
                let (to, l) = local_moves[0];
                remote_old
                    .as_ref()
                    .filter(|r| {
                        r.id == b.remote.id
                            && r.content_eq(&b.remote)
                            && l.content_eq(&b.local)
                            && single(remote, to).is_none()
                    })
                    .map(|r| {
                        (
                            to.as_str(),
                            Action::MoveRemote {
                                from: b.path.clone(),
                            },
                            l.clone(),
                            r.clone(),
                        )
                    })
            } else if remote_old.is_none() && remote_moves.len() == 1 && local_moves.is_empty() {
                let (to, items) = remote_moves[0];
                local_old
                    .filter(|l| {
                        l.content_eq(&b.local)
                            && items[0].content_eq(&b.remote)
                            && !local.entries.contains_key(to)
                    })
                    .map(|l| {
                        (
                            to.as_str(),
                            Action::MoveLocal {
                                from: b.path.clone(),
                            },
                            l.clone(),
                            items[0].clone(),
                        )
                    })
            } else {
                None
            };
        if let Some((to, action, l, r)) = move_candidate {
            let subtree_ok = baselines
                .iter()
                .filter(|c| beneath(&c.path, &b.path))
                .all(|c| {
                    let new_path = format!("{to}{}", &c.path[b.path.len()..]);
                    let (lp, rp) = if matches!(action, Action::MoveRemote { .. }) {
                        (new_path.as_str(), c.path.as_str())
                    } else {
                        (c.path.as_str(), new_path.as_str())
                    };
                    local
                        .entries
                        .get(lp)
                        .is_some_and(|v| v.content_eq(&c.local))
                        && single(remote, rp)
                            .is_some_and(|v| v.id == c.remote.id && v.content_eq(&c.remote))
                });
            let source_count = baselines
                .iter()
                .filter(|c| beneath(&c.path, &b.path))
                .count();
            let local_count = local
                .entries
                .keys()
                .filter(|p| {
                    beneath(
                        p,
                        if matches!(action, Action::MoveRemote { .. }) {
                            to
                        } else {
                            &b.path
                        },
                    )
                })
                .count();
            let remote_count = remote
                .entries
                .keys()
                .filter(|p| {
                    beneath(
                        p,
                        if matches!(action, Action::MoveRemote { .. }) {
                            &b.path
                        } else {
                            to
                        },
                    )
                })
                .count();
            if !ignore.excludes_kind(to, l.kind)?
                && !blocked
                    .iter()
                    .any(|p| beneath(p, &b.path) || beneath(p, to))
                && subtree_ok
                && source_count == local_count
                && source_count == remote_count
            {
                result
                    .operations
                    .push(operation(pair, to, action, Some(&l), Some(&r)));
                consumed.extend(
                    local
                        .entries
                        .keys()
                        .chain(remote.entries.keys())
                        .chain(base.keys())
                        .filter(|p| beneath(p, &b.path) || beneath(p, to))
                        .cloned(),
                );
                continue;
            }
        }
        if local_old.is_none() && !local_moves.is_empty()
            || remote_old.is_none() && !remote_moves.is_empty()
        {
            result.conflicts.push(conflict(
                pair,
                &b.path,
                "A move overlaps another change. Both versions are preserved.",
                local_old,
                remote_old.as_ref(),
            ));
            blocked.insert(b.path.clone());
            for (p, _) in local_moves {
                blocked.insert(p.clone());
            }
            for (p, _) in remote_moves {
                blocked.insert(p.clone());
            }
        }
    }
    let paths: BTreeSet<_> = local
        .entries
        .keys()
        .chain(remote.entries.keys())
        .chain(base.keys())
        .cloned()
        .collect();
    for path in paths {
        crate::local::validate_relative(&path)?;
        if consumed.contains(&path)
            || ignore.excludes_kind(
                &path,
                local
                    .entries
                    .get(&path)
                    .map(|l| l.kind)
                    .or_else(|| single(remote, &path).map(|r| r.kind))
                    .or_else(|| base.get(&path).map(|b| b.local.kind))
                    .unwrap_or(ItemKind::File),
            )?
            || blocked.iter().any(|p| beneath(&path, p))
        {
            continue;
        }
        let l = local.entries.get(&path);
        let r = single(remote, &path);
        let baseline = base.get(&path).copied();
        match (l, r.as_ref(), baseline) {
            (Some(l), Some(r), _) if r.matches_local(l) && baseline.is_none_or(|b| b.remote.id == r.id) => accept(&mut result, &path, l, r),
            (Some(l), Some(r), Some(b)) if l.kind == r.kind && r.id == b.remote.id => {
                match (l.content_eq(&b.local), r.content_eq(&b.remote)) {
                    (false, true) => result.operations.push(operation(pair, &path, Action::Upload, Some(l), Some(r))),
                    (true, false) => result.operations.push(operation(pair, &path, Action::Download, Some(l), Some(r))),
                    _ => { result.conflicts.push(conflict(pair, &path, "Local and Drive content both changed. Both versions are preserved.", Some(l), Some(r))); blocked.insert(path); }
                }
            }
            (Some(l), Some(r), _) => { result.conflicts.push(conflict(pair, &path, "Initial content or item identities do not match. Names and timestamps alone cannot resolve this.", Some(l), Some(r))); blocked.insert(path); }
            (Some(l), None, None) => result.operations.push(operation(pair, &path, if l.kind == ItemKind::Folder { Action::CreateRemoteFolder } else { Action::Upload }, Some(l), None)),
            (None, Some(r), None) => result.operations.push(operation(pair, &path, if r.kind == ItemKind::Folder { Action::CreateLocalFolder } else { Action::Download }, None, Some(r))),
            (None, Some(r), Some(b)) if r.id == b.remote.id && r.content_eq(&b.remote) => result.operations.push(operation(pair, &path, Action::TrashRemote, None, Some(r))),
            (Some(l), None, Some(b)) if remote.confirmed_removed.contains(&b.remote.id) && l.content_eq(&b.local) => result.operations.push(operation(pair, &path, Action::RecycleLocal, Some(l), Some(&b.remote))),
            (None, None, Some(b)) if remote.confirmed_removed.contains(&b.remote.id) => result.forgotten.push(path),
            (l, r, Some(_)) => result.conflicts.push(conflict(pair, &path, "Deletion or lost access overlaps an edit, or remote removal is unconfirmed. No deletion is inferred.", l, r)),
            _ => (),
        }
    }
    // Never trash a directory containing new/edited/conflicting descendants.
    let unsafe_deletions: Vec<_> = result
        .operations
        .iter()
        .filter(|op| {
            matches!(op.action, Action::TrashRemote | Action::RecycleLocal)
                && op
                    .expected_local
                    .as_ref()
                    .is_some_and(|l| l.kind == ItemKind::Folder)
                || matches!(op.action, Action::TrashRemote)
                    && op
                        .expected_remote
                        .as_ref()
                        .is_some_and(|r| r.kind == ItemKind::Folder)
        })
        .filter(|op| {
            let children: Vec<_> = local
                .entries
                .keys()
                .chain(remote.entries.keys())
                .filter(|p| *p != &op.path && beneath(p, &op.path))
                .collect();
            local.skipped.iter().any(|s| beneath(&s.path, &op.path))
                || result.conflicts.iter().any(|c| beneath(&c.path, &op.path))
                || children.iter().any(|p| {
                    !result.operations.iter().any(|c| {
                        &c.path == *p
                            && matches!(c.action, Action::TrashRemote | Action::RecycleLocal)
                    })
                })
        })
        .map(|op| op.path.clone())
        .collect();
    for path in &unsafe_deletions {
        result.conflicts.push(conflict(
            pair,
            path,
            "This folder contains changes or untracked content. Its deletion is stopped.",
            local.entries.get(path),
            single(remote, path).as_ref(),
        ));
    }
    result
        .operations
        .retain(|op| !unsafe_deletions.iter().any(|p| beneath(&op.path, p)));
    result.operations.sort_by_key(|op| {
        let deleting = matches!(op.action, Action::TrashRemote | Action::RecycleLocal);
        (
            deleting,
            if deleting {
                usize::MAX - op.path.matches('/').count()
            } else {
                op.path.matches('/').count()
            },
            op.path.clone(),
        )
    });
    ignore.verify_unchanged()?;
    Ok(result)
}

/// Safe UI/CLI projection: upload session URLs and private provider tokens stay in the journal.
pub fn preview(plan: &Plan) -> serde_json::Value {
    serde_json::json!({ "counts": { "operations": plan.operations.len(), "conflicts": plan.conflicts.len(), "skipped": plan.skipped.len(), "matched": plan.accepted.len() },
        "operations": plan.operations.iter().map(|o| serde_json::json!({"path":o.path,"action":o.action,"reason":match o.action { Action::Upload=>"Local content is new or changed.",Action::Download=>"Drive content is new or changed.",Action::CreateRemoteFolder=>"Create the local folder in Drive.",Action::CreateLocalFolder=>"Create the Drive folder locally.",Action::MoveRemote{..}=>"A stable local identity moved with unchanged content.",Action::MoveLocal{..}=>"A stable Drive ID moved with unchanged content.",Action::TrashRemote=>"Local removal is confirmed; retain the Drive item in Trash.",Action::RecycleLocal=>"Drive removal is confirmed; retain local content in recovery."}})).collect::<Vec<_>>(),
        "conflicts":plan.conflicts.iter().map(|c| serde_json::json!({"path":c.path,"reason":c.reason})).collect::<Vec<_>>(), "matched":plan.accepted.iter().map(|b|serde_json::json!({"path":b.path,"reason":if b.local.kind==ItemKind::Folder{"Folder identities and kinds agree."}else{"File size and content checksum agree."}})).collect::<Vec<_>>(), "skipped":plan.skipped })
}
