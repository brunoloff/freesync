//! Disposable fixture driver. Every mutation uses the private authorized test mapping.
use freesync_core::{
    db::Database,
    profile::default_profile,
    provider::{UploadProgress, UploadRequest},
    *,
};
use freesync_google::GoogleDrive;
use std::io::{Read, Seek, SeekFrom};
#[tokio::main]
async fn main() {
    if let Err(e) = run().await {
        println!("{}", serde_json::json!({"error":e}));
        std::process::exit(1);
    }
}
async fn run() -> Result<()> {
    let args: Vec<_> = std::env::args().skip(1).collect();
    let usage = || {
        Error::new(
            ErrorCode::InvalidConfig,
            "Use folder PARENT NAME, upload PARENT NAME SOURCE [EXISTING_ID], move ID PARENT NAME, trash ID, download ID DESTINATION or inspect ID.",
        )
    };
    let profile = default_profile();
    // A fixture is an independent cloud actor, not a second engine. It only reads
    // the private pair configuration and cannot replace the engine's journal.
    let db = Database::open(&profile)?;
    let pair = db
        .pairs()?
        .into_iter()
        .find(|p| p.id == "test-freesync")
        .ok_or_else(usage)?;
    let cloud = GoogleDrive::for_test_pair(&pair).await?;
    let op = uuid::Uuid::new_v4().to_string();
    let item = match args.first().map(String::as_str) {
        Some("folder") if args.len() == 3 => {
            cloud
                .create_folder(&args[1], &args[2], &op, &cloud.reserve_id().await?)
                .await?
        }
        Some("upload") if args.len() == 4 || args.len() == 5 => {
            let source = std::path::Path::new(&args[3]);
            let fingerprint = freesync_core::local::fingerprint(source)?;
            let existing = if args.len() == 5 {
                Some(cloud.get(&args[4]).await?)
            } else {
                None
            };
            let mut session = cloud
                .start_upload(&UploadRequest {
                    operation_id: op,
                    parent_id: args[1].clone(),
                    name: args[2].clone(),
                    existing,
                    size: fingerprint.size,
                    reserved_id: Some(cloud.reserve_id().await?),
                })
                .await
                .map_err(|mut e| {
                    e.message = format!("Starting fixture upload: {}", e.message);
                    e
                })?;
            loop {
                let mut input = std::fs::File::open(source)?;
                input.seek(SeekFrom::Start(session.uploaded_bytes))?;
                let mut bytes =
                    vec![0; (fingerprint.size - session.uploaded_bytes).min(1024 * 1024) as usize];
                input.read_exact(&mut bytes)?;
                match cloud.upload_chunk(&session, bytes).await.map_err(|mut e| {
                    e.message = format!("Transferring fixture chunk: {}", e.message);
                    e
                })? {
                    UploadProgress::Continue(n) => session.uploaded_bytes = n,
                    UploadProgress::Complete(item) => {
                        if item.fingerprint != Some(fingerprint) {
                            return Err(Error::new(
                                ErrorCode::Conflict,
                                "Fixture upload checksum failed.",
                            ));
                        }
                        break item;
                    }
                }
            }
        }
        Some("move") if args.len() == 4 => {
            cloud
                .move_item(&cloud.get(&args[1]).await?, &args[2], &args[3])
                .await?
        }
        Some("trash") if args.len() == 2 => cloud.trash(&cloud.get(&args[1]).await?).await?,
        Some("inspect") if args.len() == 2 => cloud.get(&args[1]).await?,
        Some("download") if args.len() == 3 => {
            let item = cloud.get(&args[1]).await?;
            if freesync_core::remote::ancestry(&cloud, &item.id, &pair.remote_root_id).await?
                != freesync_core::remote::Ancestry::Inside
            {
                return Err(Error::new(
                    ErrorCode::UnsafePath,
                    "Inspect only the disposable test fixture.",
                ));
            }
            let fingerprint = item.fingerprint.as_ref().ok_or_else(usage)?;
            let mut data = vec![];
            while data.len() < (fingerprint.size as usize) {
                let next = cloud
                    .download(&item.id, data.len() as u64, 1024 * 1024)
                    .await?;
                if next.is_empty() {
                    return Err(Error::new(
                        ErrorCode::Transient,
                        "Fixture download was truncated.",
                    ));
                }
                data.extend(next);
            }
            std::fs::write(&args[2], data)?;
            if freesync_core::local::fingerprint(std::path::Path::new(&args[2]))? != *fingerprint {
                return Err(Error::new(
                    ErrorCode::Conflict,
                    "Fixture download checksum failed.",
                ));
            }
            item
        }
        _ => return Err(usage()),
    };
    println!("{}", serde_json::to_string(&item)?);
    Ok(())
}
