//! Live migration check. All local and Drive mutations stay under the authorized test mapping.
use freesync_core::{
    adoption::{Manifest, Source},
    db::Database,
    engine::{self, EngineCommand, EngineRequest},
    profile,
    provider::{UploadProgress, UploadRequest},
    *,
};
use freesync_google::{GoogleDrive, GoogleFactory};
use std::{sync::Arc, time::Duration};
use tokio_util::sync::CancellationToken;
async fn upload(p: &dyn Provider, parent: &str, name: &str, bytes: &[u8]) -> Result<RemoteItem> {
    let session = p
        .start_upload(&UploadRequest {
            operation_id: uuid::Uuid::new_v4().to_string(),
            parent_id: parent.into(),
            name: name.into(),
            existing: None,
            size: bytes.len() as u64,
            reserved_id: Some(p.reserve_id().await?),
        })
        .await?;
    match p.upload_chunk(&session, bytes.to_vec()).await? {
        UploadProgress::Complete(item) => Ok(item),
        _ => Err(Error::new(
            ErrorCode::IncompleteScan,
            "The small fixture upload did not finish.",
        )),
    }
}
#[tokio::main]
async fn main() {
    if let Err(e) = run().await {
        println!("{}", serde_json::json!({"error":e}));
        std::process::exit(1);
    }
}
async fn run() -> Result<()> {
    let main = Database::open(&profile::default_profile())?;
    if !main.controls()?.paused {
        return Err(Error::new(
            ErrorCode::Conflict,
            "Pause the normal test engine before running this live fixture.",
        ));
    }
    let test = main
        .pairs()?
        .into_iter()
        .find(|p| p.id == "test-freesync")
        .ok_or_else(|| {
            Error::new(
                ErrorCode::InvalidConfig,
                "Configure the test mapping first.",
            )
        })?;
    let cloud = Arc::new(GoogleDrive::for_test_pair(&test).await?);
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.first().is_some_and(|s| s == "recheck") {
        let directory = std::path::PathBuf::from(args.get(1).ok_or_else(|| {
            Error::new(
                ErrorCode::InvalidConfig,
                "Supply the disposable fixture profile.",
            )
        })?);
        let _lock = profile::ProfileLock::acquire(&directory.join("adoption"))?;
        let manifest = Manifest::open(&directory.join("adoption"))?;
        let source = manifest.source()?;
        if !local::canonical_root(&source.local_root)?
            .starts_with(local::canonical_root(&test.local_root)?)
            || freesync_core::remote::ancestry(
                cloud.as_ref(),
                &source.remote_root_id,
                &test.remote_root_id,
            )
            .await?
                != freesync_core::remote::Ancestry::Inside
        {
            return Err(Error::new(
                ErrorCode::UnsafePath,
                "Recheck only a disposable migration fixture under the authorized test mapping.",
            ));
        }
        manifest.inventory_folder(cloud.as_ref()).await?;
        manifest
            .run(cloud.as_ref(), &CancellationToken::new())
            .await?;
        let report = manifest.report(None, "", 0, 50)?;
        println!(
            "{}",
            serde_json::json!({"ready":true,"counts":report.counts,"revision":report.progress.revision})
        );
        return Ok(());
    }

    let name = format!(
        "migration-{}",
        &uuid::Uuid::new_v4().simple().to_string()[..10]
    );
    let state = profile::default_profile()
        .join("live-migration-checks")
        .join(&name);
    profile::private_directory(&state)?;
    let local = test.local_root.join(&name);
    std::fs::create_dir(&local)?;
    std::fs::create_dir(local.join("scope"))?;
    let remote = cloud
        .create_folder(
            &test.remote_root_id,
            &name,
            &uuid::Uuid::new_v4().to_string(),
            &cloud.reserve_id().await?,
        )
        .await?;
    let folder = cloud
        .create_folder(
            &remote.id,
            "scope",
            &uuid::Uuid::new_v4().to_string(),
            &cloud.reserve_id().await?,
        )
        .await?;
    std::fs::write(
        state.join("fixture.json"),
        serde_json::to_vec_pretty(
            &serde_json::json!({"fixture_name":name,"local_root":local,"remote_root_id":remote.id}),
        )?,
    )?;
    std::fs::write(local.join("scope/same.txt"), b"unchanged migration bytes\n")?;
    let same = upload(
        cloud.as_ref(),
        &folder.id,
        "same.txt",
        b"unchanged migration bytes\n",
    )
    .await?;
    std::fs::write(
        local.join("scope/different.txt"),
        b"local migration bytes\n",
    )?;
    let different = upload(
        cloud.as_ref(),
        &folder.id,
        "different.txt",
        b"Drive migration bytes\n",
    )
    .await?;
    let manifest = Manifest::open(&state.join("adoption"))?;
    manifest.initialize(&Source {
        account_email: test.account_email.clone(),
        local_root: local.clone(),
        remote_root_id: remote.id.clone(),
        root_identity: local::identity(&std::fs::metadata(&local)?).unwrap(),
        excludes: vec![],
        exclusion_source: "Disposable live fixture; no ignore rules".into(),
    })?;
    manifest.inventory_folder(cloud.as_ref()).await?;
    let token = CancellationToken::new();
    token.cancel();
    assert_eq!(
        manifest.run(cloud.as_ref(), &token).await.unwrap_err().code,
        ErrorCode::Cancelled
    );
    manifest
        .run(cloud.as_ref(), &CancellationToken::new())
        .await?;
    let report = manifest.report(None, "", 0, 50)?;
    assert_eq!(report.counts["matched"], 2);
    assert_eq!(report.counts["unresolved"], 1);
    assert!(!report.counts.contains_key("upload"));
    assert!(!report.counts.contains_key("download"));
    let cancel = CancellationToken::new();
    let worker_token = cancel.clone();
    let directory = state.clone();
    let (tx, rx) = tokio::sync::mpsc::channel(2);
    let worker = std::thread::spawn(move || -> Result<()> {
        let _owner = profile::ProfileLock::acquire(&directory)?;
        let mut db = Database::open(&directory)?;
        tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()?
            .block_on(engine::run_controlled(
                &mut db,
                &directory,
                &GoogleFactory {
                    profile: directory.clone(),
                },
                worker_token,
                rx,
            ))
    });
    let (reply, response) = tokio::sync::oneshot::channel();
    tx.send(EngineRequest {
        command: EngineCommand::Adopt {
            scope: "scope".into(),
            revision: report.progress.revision,
        },
        reply,
    })
    .await
    .map_err(|_| Error::new(ErrorCode::Internal, "The fixture owner stopped."))?;
    let activated = response
        .await
        .map_err(|_| Error::new(ErrorCode::Internal, "No fixture activation result."))??;
    let pair_id = activated["pair_id"].as_str().unwrap();
    let db = Database::open(&state)?;
    let pair = db.pairs()?.into_iter().find(|p| p.id == pair_id).unwrap();
    assert!(!pair.test_only);
    assert!(pair.enabled);
    assert_eq!(db.baselines(pair_id)?[0].remote.id, same.id);
    assert_eq!(db.conflicts(pair_id)?.len(), 1);
    assert!(db.operations(pair_id)?.is_empty());
    let adopted = GoogleDrive::for_pair(&pair, &state).await?;
    assert!(
        adopted
            .create_folder(
                &test.remote_root_id,
                "outside-scope",
                "forbidden",
                &adopted.reserve_id().await?
            )
            .await
            .is_err()
    );
    std::fs::write(
        pair.local_root.join("roundtrip.txt"),
        b"new upload after reviewed adoption\n",
    )?;
    let until = std::time::Instant::now() + Duration::from_secs(90);
    let roundtrip = loop {
        if let Some(b) = db
            .baselines(pair_id)?
            .into_iter()
            .find(|b| b.path == "roundtrip.txt")
        {
            break b;
        }
        if std::time::Instant::now() > until {
            cancel.cancel();
            worker.join().unwrap()?;
            return Err(Error::new(
                ErrorCode::IncompleteScan,
                "The adopted scope did not finish its transfer.",
            ));
        }
        tokio::time::sleep(Duration::from_millis(500)).await;
    };
    cancel.cancel();
    worker.join().map_err(|_| {
        Error::new(
            ErrorCode::Internal,
            "The fixture worker stopped unexpectedly.",
        )
    })??;
    let contents = cloud.children(&folder.id, None).await?;
    assert_eq!(
        contents
            .items
            .iter()
            .filter(|i| i.name == "same.txt")
            .count(),
        1
    );
    assert_eq!(cloud.get(&same.id).await?.fingerprint, same.fingerprint);
    assert_eq!(
        cloud.get(&different.id).await?.fingerprint,
        different.fingerprint
    );
    assert_eq!(
        std::fs::read(pair.local_root.join("different.txt"))?,
        b"local migration bytes\n"
    );
    assert_eq!(
        cloud.download(&roundtrip.remote.id, 0, 1024).await?,
        b"new upload after reviewed adoption\n"
    );
    println!(
        "{}",
        serde_json::json!({"verified":true,"fixture_name":name,"profile":state,"local_root":local,"remote_root_id":remote.id,"adopted_pair_id":pair.id,"counts":report.counts,"unchanged_id_preserved":same.id,"roundtrip_id":roundtrip.remote.id,"mismatch_preserved":true,"scope_guard_verified":true,"cancel_resume_verified":true,"backup":activated["backup"]})
    );
    Ok(())
}
