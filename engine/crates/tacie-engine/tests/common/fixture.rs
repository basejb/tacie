//! An index built from `CHUNKS`/`META`, and an engine with fake services.
//! Chunk i's vector is the unit vector e_i, so multiplied by `FakeEmbed`'s question vector the dense order is
//! always 0, 1, 2, 3.

#![allow(dead_code)] // 테스트 파일마다 쓰는 것이 달라요

use std::collections::{HashMap, HashSet};
use std::path::Path;
use std::sync::{Arc, Mutex};
use tacie_sources::notion::NotionSource;

use serde_json::{Value, json};
use tacie_embed::DIM;
use tacie_engine::jev::Jev;
use tacie_engine::pipeline::Engine;
use tacie_engine::refresh::{OpenIndex, Places, Refresher, Sources};
use tacie_engine::services::Services;
use tacie_engine::settings;
use tacie_index::{Index, Mask, npy};
use tacie_sources::github::GitHub;
use tacie_sources::testing::FakeSlack;
use tacie_sources::testing::github::PanicGhToken;

use super::fakes::{FakeAnswerer, FakeEmbed, FakeReranker, ScriptedPanel, services};

pub fn chunks() -> Value {
    json!([
        {"src": "code", "path": "apps/api/src/mail/session.ts", "line": 41,
         "text": "file: apps/api/src/mail/session.ts\nsymbols: createSession, refresh\nexport function createSession() {\n  return 1;\n}"},
        {"src": "code", "path": "apps/api/src/mail/session.ts", "line": 71,
         "text": "file: apps/api/src/mail/session.ts\nexport function refresh() {}"},
        {"src": "docs", "path": "docs/메일/03_로그인.md", "line": 10,
         "text": "doc: docs/메일/03_로그인.md\nsection: 메일 > 401이 날 때\n## 401이 날 때\n기동 순서를 확인해요."},
        {"src": "code", "path": "apps/web/src/mail/Tab.tsx", "line": 1, "text": "file: apps/web/src/mail/Tab.tsx\nexport const Tab = 1;"},
    ])
}

pub fn meta() -> Value {
    json!({"repo": "acme/widgets", "commit": "abc123"})
}

/// 조각 i의 벡터는 단위 벡터 e_i예요. FakeEmbed의 질문 벡터와 곱하면 dense 순서가 0, 1, 2, 3이에요.
pub fn index_of(chunks: &Value, meta: &Value) -> Index {
    let dir = tempfile::tempdir().unwrap();
    let n = chunks.as_array().unwrap().len();
    std::fs::write(dir.path().join("chunks.json"), chunks.to_string()).unwrap();
    let mut emb = vec![0f32; n * DIM];
    for i in 0..n {
        emb[i * DIM + i] = 1.0;
    }
    npy::write_f32_2d(&dir.path().join("emb.npy"), &emb, n, DIM).unwrap();
    std::fs::write(dir.path().join("meta.json"), meta.to_string()).unwrap();
    Index::open(dir.path(), Mask::Plans, &HashSet::new()).unwrap()
}

pub fn index() -> Index {
    index_of(&chunks(), &meta())
}

/// 기본 설정(gate 0.7, show 0.8, end 0.8)과 키 파일 없는 Jev로 만든 엔진이에요.
pub fn engine_with(ix: Index, services: Services) -> (Engine, Arc<FakeEmbed>) {
    let env = HashMap::new();
    let settings = settings::resolve(&env);
    let last = Arc::new(Mutex::new(HashMap::new()));
    // 키 파일이 없는 설정 폴더예요: /health, /config용 Jev는 여기서 부르지 않아요. 엔진도 같은 폴더를 봐서
    // 진짜 ~/.config의 키를 읽지 않아요.
    let config = Path::new("/nonexistent-tacie-config");
    let jev = Arc::new(Jev::from_config(config, &settings, &env, last.clone()));
    let embed = Arc::new(FakeEmbed::default());
    let engine = Engine::new(ix, embed.clone(), services, settings, jev, last)
        .with_config_dir(config.to_path_buf());
    (engine, embed)
}

/// 연결이 없는 GitHub이에요: 아무것도 묻지 않아요(GitHub 경로를 보지 않는 서버 테스트용). gh 토큰은 부르면
/// 패닉하고, 주소는 아무도 듣지 않는 곳이에요.
pub fn no_github() -> Arc<GitHub> {
    Arc::new(GitHub::new("none", "http://127.0.0.1:9/", Arc::new(PanicGhToken)).unwrap())
}

/// `/index`가 읽는 갱신기예요(상태 idle).
/// 데이터 자리는 `index_dir/data`이고, 슬랙은 쓰지 않는 가짜예요.
pub fn refresher_for(engine: Arc<Engine>, index_dir: &Path, github: Arc<GitHub>) -> Arc<Refresher> {
    let open_index: OpenIndex = Arc::new(|d: &Path| Index::open(d, Mask::Plans, &HashSet::new()));
    Arc::new(Refresher::new(
        engine,
        index_dir.to_path_buf(),
        Places::new(&index_dir.join("data")),
        Sources {
            github,
            slack: FakeSlack::fails("slack is not used here"),
            notion: Arc::new(NotionSource::default()),
            linear: Arc::new(tacie_sources::linear::LinearSource::default()),
            gdrive: Arc::new(tacie_sources::gdrive::GDriveSource::default()),
            atlassian: Arc::new(tacie_sources::atlassian::AtlassianSource::default()),
            asana: Arc::new(tacie_sources::asana::AsanaSource::default()),
            dropbox: Arc::new(tacie_sources::dropbox::DropboxSource::default()),
        },
        open_index,
    ))
}

/// 채팅 가짜 모델과 `meetings.json`(회의 목록)을 가진 엔진이에요. 색인은 기본 색인이라 회의 조각이 없어요.
/// 돌려주는 TempDir이 회의 폴더이고, 지우면 파일도 없어져요.
pub fn harness_engine(meetings: &[Value], panel: Arc<ScriptedPanel>, iphone: bool) -> (Engine, tempfile::TempDir) {
    harness_engine_on(index(), meetings, panel, iphone)
}

/// `harness_engine`과 같은데 색인을 골라요(회의 조각이 든 색인 같은 것).
pub fn harness_engine_on(ix: Index, meetings: &[Value], panel: Arc<ScriptedPanel>, iphone: bool) -> (Engine, tempfile::TempDir) {
    let mut s = services(Arc::new(FakeReranker::all(0.0)), Arc::new(FakeAnswerer::ok("x")));
    s.panel = panel;
    let (engine, _) = engine_with(ix, s);
    let dir = tempfile::tempdir().unwrap();
    if !meetings.is_empty() {
        let list: Vec<tacie_sources::meeting::Meeting> =
            meetings.iter().map(|m| serde_json::from_value(m.clone()).unwrap()).collect();
        tacie_sources::meeting::save(dir.path(), &list).unwrap();
    }
    (engine.with_iphone(iphone), dir)
}
