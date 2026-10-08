use crate::{
    Baseline, Conflict, Controls, Error, ErrorCode, Operation, PairConfig, PairStatus, Plan, Result,
};
use rusqlite::{Connection, OptionalExtension, params};
use serde::{Serialize, de::DeserializeOwned};
use std::{path::Path, time::Duration};

pub struct Database {
    connection: Connection,
}
impl Database {
    pub fn open(directory: &Path) -> Result<Self> {
        crate::profile::private_directory(directory)?;
        let mut connection = Connection::open(directory.join("state.sqlite3"))?;
        connection.busy_timeout(Duration::from_secs(5))?;
        connection.execute_batch("PRAGMA journal_mode=WAL; PRAGMA foreign_keys=ON;")?;
        let version: i64 = connection.query_row("PRAGMA user_version", [], |r| r.get(0))?;
        if version > 1 {
            return Err(Error::new(
                ErrorCode::Database,
                "This profile was created by a newer FreeSync version.",
            ));
        }
        if version == 0 {
            let transaction = connection.transaction()?;
            transaction.execute_batch("CREATE TABLE pairs(id TEXT PRIMARY KEY, json TEXT NOT NULL);
                CREATE TABLE baselines(pair TEXT NOT NULL REFERENCES pairs(id) ON DELETE CASCADE,path TEXT NOT NULL,json TEXT NOT NULL,PRIMARY KEY(pair,path));
                CREATE TABLE operations(id TEXT PRIMARY KEY,pair TEXT NOT NULL REFERENCES pairs(id) ON DELETE CASCADE,state TEXT NOT NULL,json TEXT NOT NULL);
                CREATE INDEX operation_pair ON operations(pair,state);
                CREATE TABLE conflicts(id TEXT PRIMARY KEY,pair TEXT NOT NULL REFERENCES pairs(id) ON DELETE CASCADE,path TEXT NOT NULL,json TEXT NOT NULL,UNIQUE(pair,path));
                CREATE TABLE state(key TEXT PRIMARY KEY,json TEXT NOT NULL);
                PRAGMA user_version=1;")?;
            transaction.commit()?;
        }
        Ok(Self { connection })
    }
    pub fn schema_version(&self) -> Result<u32> {
        Ok(self
            .connection
            .query_row("PRAGMA user_version", [], |r| r.get(0))?)
    }
    pub fn save_pair(&self, pair: &PairConfig) -> Result<()> {
        self.connection.execute(
            "INSERT INTO pairs VALUES(?1,?2) ON CONFLICT(id) DO UPDATE SET json=excluded.json",
            params![pair.id, serde_json::to_string(pair)?],
        )?;
        Ok(())
    }
    pub fn pairs(&self) -> Result<Vec<PairConfig>> {
        self.all_json("SELECT json FROM pairs ORDER BY id", [])
    }
    pub fn baselines(&self, pair: &str) -> Result<Vec<Baseline>> {
        self.all_json(
            "SELECT json FROM baselines WHERE pair=?1 ORDER BY path",
            [pair],
        )
    }
    pub fn operations(&self, pair: &str) -> Result<Vec<Operation>> {
        self.all_json("SELECT json FROM operations WHERE pair=?1 AND state NOT IN ('done','conflict') ORDER BY rowid",[pair])
    }
    pub fn conflicts(&self, pair: &str) -> Result<Vec<Conflict>> {
        self.all_json(
            "SELECT json FROM conflicts WHERE pair=?1 ORDER BY path",
            [pair],
        )
    }
    fn all_json<T: DeserializeOwned, P: rusqlite::Params>(
        &self,
        sql: &str,
        params: P,
    ) -> Result<Vec<T>> {
        let mut stmt = self.connection.prepare(sql)?;
        let values = stmt.query_map(params, |r| r.get::<_, String>(0))?;
        values.map(|s| Ok(serde_json::from_str(&s?)?)).collect()
    }
    pub fn set<T: Serialize>(&self, key: &str, value: &T) -> Result<()> {
        self.connection.execute(
            "INSERT INTO state VALUES(?1,?2) ON CONFLICT(key) DO UPDATE SET json=excluded.json",
            params![key, serde_json::to_string(value)?],
        )?;
        Ok(())
    }
    pub fn get<T: DeserializeOwned>(&self, key: &str) -> Result<Option<T>> {
        let value: Option<String> = self
            .connection
            .query_row("SELECT json FROM state WHERE key=?1", [key], |r| r.get(0))
            .optional()?;
        value
            .map(|v| serde_json::from_str(&v).map_err(Into::into))
            .transpose()
    }
    pub fn controls(&self) -> Result<Controls> {
        Ok(self.get("controls")?.unwrap_or_default())
    }
    pub fn save_operation(&self, operation: &Operation) -> Result<()> {
        let state = serde_json::to_value(operation.state)?
            .as_str()
            .unwrap()
            .to_owned();
        self.connection.execute("INSERT INTO operations VALUES(?1,?2,?3,?4) ON CONFLICT(id) DO UPDATE SET state=excluded.state,json=excluded.json",
            params![operation.id,operation.pair_id,state,serde_json::to_string(operation)?])?;
        Ok(())
    }
    pub fn set_status(&self, status: &PairStatus) -> Result<()> {
        self.set(&format!("status:{}", status.pair_id), status)
    }
    pub fn status(&self, pair: &str) -> Result<Option<PairStatus>> {
        self.get(&format!("status:{pair}"))
    }
    /// Inventories, work and consumed change cursor form one durable transaction.
    pub fn persist_plan(
        &mut self,
        pair: &str,
        plan: &Plan,
        local: &crate::LocalInventory,
        remote: &crate::RemoteInventory,
    ) -> Result<()> {
        if !self.operations(pair)?.is_empty() {
            return Err(Error::new(
                ErrorCode::Conflict,
                "Pending work must finish or be resolved before replacing its plan.",
            ));
        }
        let tx = self.connection.transaction()?;
        for (key, value) in [
            (format!("local:{pair}"), serde_json::to_string(local)?),
            (format!("remote:{pair}"), serde_json::to_string(remote)?),
        ] {
            tx.execute(
                "INSERT INTO state VALUES(?1,?2) ON CONFLICT(key) DO UPDATE SET json=excluded.json",
                params![key, value],
            )?;
        }
        for baseline in &plan.accepted {
            tx.execute("INSERT INTO baselines VALUES(?1,?2,?3) ON CONFLICT(pair,path) DO UPDATE SET json=excluded.json",params![pair,baseline.path,serde_json::to_string(baseline)?])?;
        }
        for path in &plan.forgotten {
            tx.execute(
                "DELETE FROM baselines WHERE pair=?1 AND path=?2",
                params![pair, path],
            )?;
        }
        tx.execute("DELETE FROM conflicts WHERE pair=?1", [pair])?;
        for conflict in &plan.conflicts {
            tx.execute(
                "INSERT INTO conflicts VALUES(?1,?2,?3,?4)",
                params![
                    conflict.id,
                    pair,
                    conflict.path,
                    serde_json::to_string(conflict)?
                ],
            )?;
        }
        for operation in &plan.operations {
            let state = serde_json::to_value(operation.state)?
                .as_str()
                .unwrap()
                .to_owned();
            tx.execute(
                "INSERT INTO operations VALUES(?1,?2,?3,?4)",
                params![operation.id, pair, state, serde_json::to_string(operation)?],
            )?;
        }
        tx.commit()?;
        Ok(())
    }
    /// Mark completion and update baseline atomically; a crash cannot lose the completed mapping.
    pub fn finish_operation(
        &mut self,
        operation: &Operation,
        baseline: Option<&Baseline>,
        remove_prefix: Option<&str>,
    ) -> Result<()> {
        self.finish_many(
            operation,
            &baseline.cloned().into_iter().collect::<Vec<_>>(),
            remove_prefix,
        )
    }
    pub fn finish_many(
        &mut self,
        operation: &Operation,
        baselines: &[Baseline],
        remove_prefix: Option<&str>,
    ) -> Result<()> {
        let tx = self.connection.transaction()?;
        if let Some(prefix) = remove_prefix {
            let paths: Vec<String> = {
                let mut q = tx.prepare("SELECT path FROM baselines WHERE pair=?1")?;
                q.query_map([&operation.pair_id], |r| r.get(0))?
                    .collect::<std::result::Result<_, _>>()?
            };
            for p in paths {
                if p == prefix || p.starts_with(&format!("{prefix}/")) {
                    tx.execute(
                        "DELETE FROM baselines WHERE pair=?1 AND path=?2",
                        params![operation.pair_id, p],
                    )?;
                }
            }
        }
        for b in baselines {
            tx.execute("INSERT INTO baselines VALUES(?1,?2,?3) ON CONFLICT(pair,path) DO UPDATE SET json=excluded.json",params![operation.pair_id,b.path,serde_json::to_string(b)?])?;
        }
        let mut finished = operation.clone();
        finished.state = crate::OperationState::Done;
        finished.upload_session = None;
        tx.execute(
            "UPDATE operations SET state='done',json=?2 WHERE id=?1",
            params![finished.id, serde_json::to_string(&finished)?],
        )?;
        tx.commit()?;
        Ok(())
    }
    pub fn conflict_operation(&mut self, operation: &Operation, reason: &str) -> Result<()> {
        let tx = self.connection.transaction()?;
        let conflict = Conflict {
            id: uuid::Uuid::new_v4().to_string(),
            pair_id: operation.pair_id.clone(),
            path: operation.path.clone(),
            reason: reason.into(),
            local: operation.expected_local.clone(),
            remote: operation.expected_remote.clone(),
        };
        tx.execute("INSERT INTO conflicts VALUES(?1,?2,?3,?4) ON CONFLICT(pair,path) DO UPDATE SET id=excluded.id,json=excluded.json",params![conflict.id,conflict.pair_id,conflict.path,serde_json::to_string(&conflict)?])?;
        let mut op = operation.clone();
        op.state = crate::OperationState::Conflict;
        tx.execute(
            "UPDATE operations SET state='conflict',json=?2 WHERE id=?1",
            params![op.id, serde_json::to_string(&op)?],
        )?;
        tx.commit()?;
        Ok(())
    }
    pub fn approve_deletion_operations(
        &mut self,
        pair: &str,
        operations: &[Operation],
    ) -> Result<()> {
        let tx = self.connection.transaction()?;
        for op in operations {
            tx.execute(
                "UPDATE operations SET state='prepared',json=?2 WHERE id=?1",
                params![op.id, serde_json::to_string(op)?],
            )?;
        }
        for (key, value) in [
            (
                format!("deletion_approved:{pair}"),
                serde_json::to_string(
                    &operations.iter().map(|o| o.id.clone()).collect::<Vec<_>>(),
                )?,
            ),
            (format!("deletion_hold:{pair}"), "false".into()),
        ] {
            tx.execute(
                "INSERT INTO state VALUES(?1,?2) ON CONFLICT(key) DO UPDATE SET json=excluded.json",
                params![key, value],
            )?;
        }
        tx.commit()?;
        Ok(())
    }
    pub fn retry_conflict(&mut self, pair: &str, path: &str) -> Result<()> {
        let mut op:Operation=self.all_json("SELECT json FROM operations WHERE pair=?1 AND state='conflict' AND json_extract(json,'$.path')=?2 ORDER BY rowid DESC LIMIT 1",[pair,path])?.into_iter().next().ok_or_else(||Error::new(ErrorCode::InvalidConfig,"This conflict needs resolution rather than a transfer retry."))?;
        op.state = crate::OperationState::Prepared;
        op.retry_at = 0;
        op.error = None;
        let tx = self.connection.transaction()?;
        tx.execute(
            "UPDATE operations SET state='prepared',json=?2 WHERE id=?1",
            params![op.id, serde_json::to_string(&op)?],
        )?;
        tx.execute(
            "DELETE FROM conflicts WHERE pair=?1 AND path=?2",
            params![pair, path],
        )?;
        tx.commit()?;
        Ok(())
    }
    pub fn finish_keep_both(&mut self, pair: &str, path: &str, conflict_id: &str) -> Result<()> {
        let tx = self.connection.transaction()?;
        tx.execute(
            "DELETE FROM baselines WHERE pair=?1 AND path=?2",
            params![pair, path],
        )?;
        tx.execute(
            "DELETE FROM conflicts WHERE pair=?1 AND path=?2 AND id=?3",
            params![pair, path, conflict_id],
        )?;
        tx.execute(
            "INSERT INTO state VALUES(?1,?2) ON CONFLICT(key) DO UPDATE SET json=excluded.json",
            params![format!("resolution_done:{conflict_id}"), "true"],
        )?;
        tx.commit()?;
        Ok(())
    }
}
