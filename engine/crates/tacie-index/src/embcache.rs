//! Keeps chunk vectors outside the index, so text already embedded is not embedded again after a rebuild or a
//! repository switch. The file is `vectors.sqlite`, and its format does not change.

use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::Duration;

use anyhow::{Result, ensure};
use rusqlite::{Connection, ErrorCode, params, params_from_iter};
use sha2::{Digest, Sha256};

/// `IN (...)`과 한 번에 넣는 줄 수예요. SQLite 변수 개수 한도보다 한참 작아요.
const STEP: usize = 500;

fn key(text: &str) -> String {
    hex::encode(Sha256::digest(text.as_bytes()))
}

/// SQLite가 파일을 데이터베이스로 읽지 못해요: 아예 아니거나 깨졌어요(잠금이나 디스크 가득 참은 아니에요).
fn unreadable(e: &rusqlite::Error) -> bool {
    match e {
        rusqlite::Error::SqliteFailure(f, msg) => {
            matches!(f.code, ErrorCode::NotADatabase | ErrorCode::DatabaseCorrupt)
                || msg.as_deref().is_some_and(|m| {
                    m.contains("file is not a database") || m.contains("malformed")
                })
        }
        _ => false,
    }
}

/// 글의 sha256으로 찾는 벡터 캐시예요. 모델과 글 길이마다 SQLite 파일 하나이고 저장소와 브랜치가 같이 써요.
/// 태그 "<repo key>#<branch>"가 누가 그 벡터를 쓰는지 표시하고, 어떤 태그도 안 쓰는 벡터는 지워요. 호출마다 새 연결이에요.
pub struct EmbCache {
    pub path: PathBuf,
    model: String,
    tokens: usize,
}

impl EmbCache {
    pub fn new(root: &Path, model: &str, tokens: usize) -> Self {
        let dir = format!("{}@{}", model.replace('/', "__"), tokens);
        Self {
            path: root.join(dir).join("vectors.sqlite"),
            model: model.to_string(),
            tokens,
        }
    }

    /// 이 캐시의 벡터를 만든 모델이에요(옛 색인의 meta `embed`와 비교해요).
    pub fn model(&self) -> &str {
        &self.model
    }

    /// 문단을 자른 토큰 수예요.
    pub fn tokens(&self) -> usize {
        self.tokens
    }

    fn connect(&self) -> rusqlite::Result<Connection> {
        if let Some(dir) = self.path.parent() {
            fs::create_dir_all(dir)
                .map_err(|e| rusqlite::Error::ToSqlConversionFailure(Box::new(e)))?;
        }
        let db = Connection::open(&self.path)?;
        db.busy_timeout(Duration::from_secs(30))?;
        db.execute(
            "CREATE TABLE IF NOT EXISTS vec (key TEXT PRIMARY KEY, v BLOB)",
            [],
        )?;
        db.execute(
            "CREATE TABLE IF NOT EXISTS tags (key TEXT, tag TEXT, PRIMARY KEY (key, tag))",
            [],
        )?;
        Ok(db)
    }

    /// 새 연결에서 op을 해요. SQLite가 못 읽는 파일은 vectors.sqlite+bad로 옮기고(예전 것은 덮어써요) 빈 캐시에서
    /// 한 번 더 해요. 벡터를 잃으면 다시 임베딩하는 비용뿐이에요.
    fn run<T>(&self, mut op: impl FnMut(&mut Connection) -> rusqlite::Result<T>) -> Result<T> {
        for attempt in 0..2 {
            let r = self.connect().and_then(|mut db| op(&mut db));
            match r {
                Ok(v) => return Ok(v),
                Err(e) if attempt == 0 && unreadable(&e) => self.move_aside(&e)?,
                Err(e) => return Err(e.into()),
            }
        }
        unreachable!("the second attempt always returns")
    }

    fn side(&self, suffix: &str) -> PathBuf {
        let mut name = self.path.file_name().unwrap_or_default().to_os_string();
        name.push(suffix);
        self.path.with_file_name(name)
    }

    fn move_aside(&self, e: &rusqlite::Error) -> Result<()> {
        let bad = self.side("+bad");
        if self.path.exists() {
            fs::rename(&self.path, &bad)?;
        }
        for s in ["-journal", "-wal", "-shm"] {
            let p = self.side(s);
            if p.exists() {
                fs::remove_file(p)?;
            }
        }
        let dir = self
            .path
            .parent()
            .and_then(|p| p.file_name())
            .unwrap_or_default()
            .to_string_lossy();
        eprintln!(
            "embedding cache: {dir}/vectors.sqlite cannot be read ({e}), moved to vectors.sqlite+bad; starting an empty one"
        );
        Ok(())
    }

    /// `texts`의 위치 -> 캐시에 있는 벡터예요. 같은 글이 여러 번이면 위치마다 같은 벡터가 들어가요.
    /// BLOB 길이가 4의 배수가 아니면 Err예요(남는 바이트를 버리고 짧은 벡터를 만들지 않아요).
    pub fn get(&self, texts: &[&str]) -> Result<HashMap<usize, Vec<f32>>> {
        let mut where_: HashMap<String, Vec<usize>> = HashMap::new();
        let mut keys: Vec<String> = Vec::new();
        for (i, t) in texts.iter().enumerate() {
            let k = key(t);
            if !where_.contains_key(&k) {
                keys.push(k.clone());
            }
            where_.entry(k).or_default().push(i);
        }
        let rows: Vec<(String, Vec<u8>)> = self.run(|db| {
            let mut rows = Vec::new();
            for batch in keys.chunks(STEP) {
                let q = vec!["?"; batch.len()].join(",");
                let mut st = db.prepare(&format!("SELECT key, v FROM vec WHERE key IN ({q})"))?;
                let mut found = st.query(params_from_iter(batch))?;
                while let Some(row) = found.next()? {
                    rows.push((row.get(0)?, row.get(1)?));
                }
            }
            Ok(rows)
        })?;
        let mut found = HashMap::new();
        for (k, b) in rows {
            let (words, rest) = b.as_chunks::<4>();
            ensure!(
                rest.is_empty(),
                "embedding cache: the vector under {k} is {} bytes, not a whole number of float32s",
                b.len()
            );
            let vec: Vec<f32> = words.iter().map(|c| f32::from_le_bytes(*c)).collect();
            for &i in &where_[&k] {
                found.insert(i, vec.clone());
            }
        }
        Ok(found)
    }

    pub fn put(&self, texts: &[&str], vecs: &[Vec<f32>]) -> Result<()> {
        let rows: Vec<(String, Vec<u8>)> = texts
            .iter()
            .zip(vecs)
            .map(|(t, v)| (key(t), v.iter().flat_map(|x| x.to_le_bytes()).collect()))
            .collect();
        self.run(|db| {
            let tx = db.transaction()?;
            {
                let mut st = tx.prepare("INSERT OR REPLACE INTO vec VALUES (?, ?)")?;
                for (k, b) in &rows {
                    st.execute(params![k, b])?;
                }
            }
            tx.commit()
        })
    }

    fn sweep(db: &Connection) -> rusqlite::Result<usize> {
        db.execute(
            "DELETE FROM vec WHERE key NOT IN (SELECT key FROM tags)",
            [],
        )
    }

    /// 태그가 정확히 이 글들만 표시하게 하고, 어떤 태그도 안 쓰는 벡터는 지워요. 지운 벡터 수를 돌려줘요.
    pub fn retag(&self, tag: &str, texts: &[&str]) -> Result<usize> {
        let mut keys: Vec<String> = texts.iter().map(|t| key(t)).collect();
        keys.sort();
        keys.dedup();
        self.run(|db| {
            let tx = db.transaction()?;
            tx.execute("DELETE FROM tags WHERE tag = ?", params![tag])?;
            {
                let mut st = tx.prepare("INSERT OR IGNORE INTO tags VALUES (?, ?)")?;
                for batch in keys.chunks(STEP) {
                    for k in batch {
                        st.execute(params![k, tag])?;
                    }
                }
            }
            let n = Self::sweep(&tx)?;
            tx.commit()?;
            Ok(n)
        })
    }

    /// `prefix`로 시작하는 태그를 모두 지우고, 어떤 태그도 안 쓰는 벡터는 지워요. 지운 벡터 수를 돌려줘요.
    pub fn drop_prefix(&self, prefix: &str) -> Result<usize> {
        let like = format!(
            "{}%",
            prefix
                .replace('\\', "\\\\")
                .replace('%', "\\%")
                .replace('_', "\\_")
        );
        self.run(|db| {
            let tx = db.transaction()?;
            tx.execute(
                "DELETE FROM tags WHERE tag LIKE ? ESCAPE '\\'",
                params![like],
            )?;
            let n = Self::sweep(&tx)?;
            tx.commit()?;
            Ok(n)
        })
    }
}
