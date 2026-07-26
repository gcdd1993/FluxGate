use crate::models::{
    GatewayConfig, GroupStat, ModelAlias, Provider, RequestLog, UsageFilter, UsagePoint,
    UsageSummary,
};
use anyhow::{Context, Result};
use chrono::{Duration, Local, Utc};
use rusqlite::{params, Connection};
use std::{collections::BTreeMap, path::Path, sync::Mutex};

pub struct Database {
    conn: Mutex<Connection>,
}

impl Database {
    pub fn open(path: &Path) -> Result<Self> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let conn = Connection::open(path).context("open FluxGate database")?;
        conn.execute_batch(
            "PRAGMA journal_mode=WAL; PRAGMA foreign_keys=ON;
          CREATE TABLE IF NOT EXISTS settings (key TEXT PRIMARY KEY, value TEXT NOT NULL);
          CREATE TABLE IF NOT EXISTS providers (id TEXT PRIMARY KEY, data TEXT NOT NULL);
          CREATE TABLE IF NOT EXISTS aliases (id TEXT PRIMARY KEY, data TEXT NOT NULL);
          CREATE TABLE IF NOT EXISTS request_logs (
            id INTEGER PRIMARY KEY AUTOINCREMENT, timestamp TEXT NOT NULL, client TEXT NOT NULL,
            provider_name TEXT NOT NULL, provider_id TEXT NOT NULL, model TEXT NOT NULL,
            input_tokens INTEGER NOT NULL DEFAULT 0, output_tokens INTEGER NOT NULL DEFAULT 0,
            cached_tokens INTEGER NOT NULL DEFAULT 0, cost REAL NOT NULL DEFAULT 0,
            latency_ms INTEGER NOT NULL DEFAULT 0, status INTEGER NOT NULL, error TEXT
          ); CREATE INDEX IF NOT EXISTS idx_logs_timestamp ON request_logs(timestamp DESC);",
        )?;
        Ok(Self {
            conn: Mutex::new(conn),
        })
    }
    /// Write a consistent snapshot of the live database to `target` using
    /// `VACUUM INTO`, which is safe while other connections keep writing.
    /// The target file must not exist yet.
    pub fn snapshot_to(&self, target: &Path) -> Result<()> {
        if let Some(parent) = target.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let target = target
            .to_str()
            .context("snapshot target path is not valid UTF-8")?;
        self.conn
            .lock()
            .expect("db lock")
            .execute("VACUUM INTO ?1", [target])
            .context("snapshot FluxGate database")?;
        Ok(())
    }

    pub fn config(&self) -> GatewayConfig {
        let conn = self.conn.lock().expect("db lock");
        conn.query_row(
            "SELECT value FROM settings WHERE key='gateway_config'",
            [],
            |r| r.get::<_, String>(0),
        )
        .ok()
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_default()
    }
    pub fn save_config(&self, value: &GatewayConfig) -> Result<()> {
        let json = serde_json::to_string(value)?;
        self.conn.lock().expect("db lock").execute("INSERT INTO settings(key,value) VALUES('gateway_config',?1) ON CONFLICT(key) DO UPDATE SET value=excluded.value", [json])?;
        Ok(())
    }
    pub fn providers(&self) -> Result<Vec<Provider>> {
        self.json_rows("SELECT data FROM providers ORDER BY json_extract(data, '$.priority'), json_extract(data, '$.name')")
    }
    pub fn aliases(&self) -> Result<Vec<ModelAlias>> {
        let mut aliases: Vec<ModelAlias> =
            self.json_rows("SELECT data FROM aliases ORDER BY json_extract(data, '$.alias')")?;
        for alias in &mut aliases {
            alias.normalize();
        }
        Ok(aliases)
    }
    fn json_rows<T: serde::de::DeserializeOwned>(&self, sql: &str) -> Result<Vec<T>> {
        let conn = self.conn.lock().expect("db lock");
        let mut stmt = conn.prepare(sql)?;
        let rows = stmt.query_map([], |r| r.get::<_, String>(0))?;
        let mut out = Vec::new();
        for row in rows {
            out.push(serde_json::from_str(&row?)?);
        }
        Ok(out)
    }
    pub fn save_provider(&self, value: &Provider) -> Result<()> {
        let json = serde_json::to_string(value)?;
        self.conn.lock().expect("db lock").execute("INSERT INTO providers(id,data) VALUES(?1,?2) ON CONFLICT(id) DO UPDATE SET data=excluded.data",params![value.id,json])?;
        Ok(())
    }
    pub fn delete_provider(&self, id: &str) -> Result<()> {
        let mut conn = self.conn.lock().expect("db lock");
        let tx = conn.transaction()?;
        let aliases = {
            let mut stmt = tx.prepare("SELECT id,data FROM aliases")?;
            let rows = stmt.query_map([], |row| {
                Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
            })?;
            rows.filter_map(Result::ok).collect::<Vec<_>>()
        };
        for (alias_id, data) in aliases {
            let mut alias: ModelAlias = serde_json::from_str(&data)?;
            alias.normalize();
            alias.targets.retain(|target| target.provider_id != id);
            if alias.targets.is_empty() {
                tx.execute("DELETE FROM aliases WHERE id=?1", [&alias_id])?;
            } else {
                alias.provider_id.clear();
                alias.upstream_model.clear();
                tx.execute(
                    "UPDATE aliases SET data=?2 WHERE id=?1",
                    params![alias_id, serde_json::to_string(&alias)?],
                )?;
            }
        }
        tx.execute("DELETE FROM providers WHERE id=?1", [id])?;
        tx.commit()?;
        Ok(())
    }
    pub fn save_alias(&self, value: &ModelAlias) -> Result<()> {
        let json = serde_json::to_string(value)?;
        self.conn.lock().expect("db lock").execute("INSERT INTO aliases(id,data) VALUES(?1,?2) ON CONFLICT(id) DO UPDATE SET data=excluded.data",params![value.id,json])?;
        Ok(())
    }
    pub fn delete_alias(&self, id: &str) -> Result<()> {
        self.conn
            .lock()
            .expect("db lock")
            .execute("DELETE FROM aliases WHERE id=?1", [id])?;
        Ok(())
    }
    pub fn insert_log(&self, l: &RequestLog) -> Result<()> {
        self.conn.lock().expect("db lock").execute("INSERT INTO request_logs(timestamp,client,provider_name,provider_id,model,input_tokens,output_tokens,cached_tokens,cost,latency_ms,status,error) VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12)", params![l.timestamp,l.client,l.provider_name,l.provider_id,l.model,l.input_tokens,l.output_tokens,l.cached_tokens,l.cost,l.latency_ms,l.status,l.error])?;
        Ok(())
    }
    pub fn logs(&self, filter: &UsageFilter, limit: i64) -> Result<Vec<RequestLog>> {
        let days = match filter.range.as_str() {
            "today" => 0,
            "30d" => 30,
            _ => 7,
        };
        let since = if days == 0 {
            Local::now()
                .date_naive()
                .and_hms_opt(0, 0, 0)
                .unwrap()
                .and_local_timezone(Local)
                .unwrap()
                .with_timezone(&Utc)
        } else {
            Utc::now() - Duration::days(days)
        };
        let conn = self.conn.lock().expect("db lock");
        let mut stmt=conn.prepare("SELECT id,timestamp,client,provider_name,provider_id,model,input_tokens,output_tokens,cached_tokens,cost,latency_ms,status,error FROM request_logs WHERE timestamp>=?1 AND (?2='all' OR client=?2) AND (?3='' OR provider_id=?3) AND (?4='' OR model LIKE '%'||?4||'%') ORDER BY timestamp DESC LIMIT ?5")?;
        let rows = stmt.query_map(
            params![
                since.to_rfc3339(),
                filter.client,
                filter.provider,
                filter.model,
                limit
            ],
            |r| {
                Ok(RequestLog {
                    id: r.get(0)?,
                    timestamp: r.get(1)?,
                    client: r.get(2)?,
                    provider_name: r.get(3)?,
                    provider_id: r.get(4)?,
                    model: r.get(5)?,
                    input_tokens: r.get(6)?,
                    output_tokens: r.get(7)?,
                    cached_tokens: r.get(8)?,
                    cost: r.get(9)?,
                    latency_ms: r.get(10)?,
                    status: r.get(11)?,
                    error: r.get(12)?,
                })
            },
        )?;
        Ok(rows.filter_map(Result::ok).collect())
    }
    pub fn analytics(
        &self,
        filter: &UsageFilter,
    ) -> Result<(
        UsageSummary,
        Vec<UsagePoint>,
        Vec<GroupStat>,
        Vec<GroupStat>,
        Vec<RequestLog>,
    )> {
        let logs = self.logs(filter, 1000)?;
        let requests = logs.len() as i64;
        let input: i64 = logs.iter().map(|x| x.input_tokens).sum();
        let output: i64 = logs.iter().map(|x| x.output_tokens).sum();
        let cached: i64 = logs.iter().map(|x| x.cached_tokens).sum();
        let success = logs.iter().filter(|x| x.status < 400).count() as f64;
        let cost = logs.iter().map(|x| x.cost).sum();
        let avg = if requests > 0 {
            logs.iter().map(|x| x.latency_ms).sum::<i64>() / requests
        } else {
            0
        };
        let summary = UsageSummary {
            requests,
            tokens: input + output + cached,
            cache_hit_rate: if input + cached > 0 {
                cached as f64 * 100.0 / (input + cached) as f64
            } else {
                0.0
            },
            cost,
            success_rate: if requests > 0 {
                success * 100.0 / requests as f64
            } else {
                100.0
            },
            avg_latency_ms: avg,
        };
        let mut points: BTreeMap<String, UsagePoint> = BTreeMap::new();
        for l in &logs {
            let label = l.timestamp.get(5..10).unwrap_or("--").replace('-', "/");
            let p = points.entry(label.clone()).or_insert(UsagePoint {
                label,
                ..Default::default()
            });
            p.input += l.input_tokens / 1000;
            p.output += l.output_tokens / 1000;
            p.cached += l.cached_tokens / 1000;
            p.cost += l.cost;
            p.requests += 1;
        }
        Ok((
            summary,
            points.into_values().collect(),
            group(&logs, true),
            group(&logs, false),
            logs,
        ))
    }
}

fn group(logs: &[RequestLog], provider: bool) -> Vec<GroupStat> {
    let mut map: BTreeMap<String, Vec<&RequestLog>> = BTreeMap::new();
    for l in logs {
        map.entry(if provider {
            l.provider_name.clone()
        } else {
            l.model.clone()
        })
        .or_default()
        .push(l);
    }
    map.into_iter()
        .map(|(name, v)| {
            let n = v.len() as i64;
            GroupStat {
                name,
                requests: n,
                tokens: v
                    .iter()
                    .map(|x| x.input_tokens + x.output_tokens + x.cached_tokens)
                    .sum(),
                success_rate: if n > 0 {
                    v.iter().filter(|x| x.status < 400).count() as f64 * 100.0 / n as f64
                } else {
                    0.0
                },
                cost: v.iter().map(|x| x.cost).sum(),
                latency_ms: if n > 0 {
                    v.iter().map(|x| x.latency_ms).sum::<i64>() / n
                } else {
                    0
                },
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn aggregates_usage_and_filters_by_client() {
        let path = std::env::temp_dir().join(format!("fluxgate-test-{}.db", uuid::Uuid::new_v4()));
        let db = Database::open(&path).unwrap();
        for (client, status) in [("codex", 200), ("claude", 500)] {
            db.insert_log(&RequestLog {
                id: 0,
                timestamp: Utc::now().to_rfc3339(),
                client: client.into(),
                provider_name: "Test".into(),
                provider_id: "p1".into(),
                model: "model-a".into(),
                input_tokens: 100,
                output_tokens: 25,
                cached_tokens: 50,
                cost: 0.01,
                latency_ms: 200,
                status,
                error: None,
            })
            .unwrap();
        }
        let filter = UsageFilter {
            range: "7d".into(),
            client: "codex".into(),
            provider: String::new(),
            model: String::new(),
        };
        let (summary, _, providers, models, logs) = db.analytics(&filter).unwrap();
        assert_eq!(summary.requests, 1);
        assert_eq!(summary.tokens, 175);
        assert_eq!(summary.success_rate, 100.0);
        assert_eq!(providers[0].requests, 1);
        assert_eq!(models[0].name, "model-a");
        assert_eq!(logs[0].client, "codex");
        drop(db);
        let _ = std::fs::remove_file(path);
    }
}
