//! Tests the pipeline (`Engine::ask`, `compose_answer`, `check_answer`) with a fake embedder, reranker and answer
//! model.
//! The index is `CHUNKS`/`META`. Real models and APIs are not called.

mod common {
    pub mod fakes;
    pub mod fixture;
}

use std::path::{Path, PathBuf};
use std::sync::Arc;

use common::fakes::{
    FakeAnswerer, FakeEmbed, FakeGater, FakeNoteWriter, FakeReranker, FakeTranscriber, services,
};
use common::fixture::{chunks, engine_with, index, index_of};
use serde_json::{Value, json};
use tacie_engine::errors::Kind;
use tacie_engine::pipeline::{Engine, Scope, check_answer, checkout_repo};
use tacie_engine::services::Services;
use tacie_index::Index;

fn make_engine(
    ix: Index,
    reranker: &Arc<FakeReranker>,
    answerer: &Arc<FakeAnswerer>,
) -> (Engine, Arc<FakeEmbed>) {
    engine_with(ix, services(reranker.clone(), answerer.clone()))
}

fn keys(v: &Value) -> Vec<&str> {
    v.as_object().unwrap().keys().map(String::as_str).collect()
}

fn column<'a>(cards: &'a Value, key: &str) -> Vec<&'a Value> {
    cards.as_array().unwrap().iter().map(|c| &c[key]).collect()
}

fn texts(chunks: &Value) -> Vec<String> {
    chunks
        .as_array()
        .unwrap()
        .iter()
        .map(|c| c["text"].as_str().unwrap().to_string())
        .collect()
}

#[tokio::test]
async fn ask_ranks_by_jev_and_applies_show_threshold() {
    let reranker = Arc::new(FakeReranker::ok(&[0.1, 0.2, 0.95, 0.3]));
    let (engine, _) = make_engine(index(), &reranker, &Arc::new(FakeAnswerer::ok("")));
    let out = engine
        .ask("메일 탭 401 어디부터 봐요?", &[], engine.settings.show)
        .await;
    assert_eq!(out["cards"][0]["path"], "docs/메일/03_로그인.md");
    assert_eq!(
        (&out["shown"], &out["best_p"], &out["low_confidence"]),
        (&json!(true), &json!(0.95), &json!(false))
    );
    // p 내림차순, 문서마다 한 장(71행 조각이 41행보다 p가 높아요)
    assert_eq!(
        column(&out["cards"], "chunk"),
        [&json!(2), &json!(3), &json!(1)]
    );
    assert_eq!(
        column(&out["cards"], "score"),
        [&json!(0.95), &json!(0.3), &json!(0.2)]
    );
    assert_eq!(
        keys(&out),
        [
            "cards",
            "best_p",
            "shown",
            "low_confidence",
            "cost_usd",
            "timing_ms"
        ]
    );
    assert_eq!(out["cost_usd"], 0.0021);
    assert_eq!(keys(&out["timing_ms"]), ["search", "rerank", "total"]);
    assert_eq!(out["timing_ms"]["rerank"], 5.0);

    // 리랭커에는 앞 대화와 "나: " + 질문, 후보 조각의 글(900자까지)이 가요. 앞 대화는 마지막 세 줄만이에요.
    let context: Vec<String> = ["a", "b", "c", "d"].map(String::from).to_vec();
    engine.ask("포트는요?", &context, 0.8).await;
    let calls = reranker.calls();
    assert_eq!(
        calls[0],
        (
            vec![],
            "나: 메일 탭 401 어디부터 봐요?".to_string(),
            texts(&chunks())
        )
    );
    assert_eq!(
        (calls[1].0.clone(), calls[1].1.as_str()),
        (
            vec!["b".to_string(), "c".into(), "d".into()],
            "나: 포트는요?"
        )
    );

    let reranker = Arc::new(FakeReranker::ok(&[0.1, 0.2, 0.5, 0.3]));
    let (low, _) = engine_pair(&reranker);
    assert_eq!(
        low.ask("아무거나", &[], low.settings.show).await["shown"],
        false
    );
}

#[tokio::test]
async fn engine_holds_all_five_services() {
    let reranker = Arc::new(FakeReranker::ok(&[0.1, 0.2, 0.95, 0.3]));
    let gater = Arc::new(FakeGater::ending(0.83, Some(0.12)));
    let transcriber = Arc::new(FakeTranscriber::ok("메일 탭 401 나면 어디 봐요?"));
    let note_writer = Arc::new(FakeNoteWriter::ok(r#"{"items": []}"#));
    let services = Services {
        reranker: reranker.clone(),
        gater: gater.clone(),
        answerer: Arc::new(FakeAnswerer::ok("")),
        transcriber: transcriber.clone(),
        note_writer: note_writer.clone(),
        panel: Arc::new(common::fakes::FakePanel),
        meeting_search: None,
    };
    let (engine, _) = engine_with(index(), services);

    // 묻기 창은 지금처럼 리랭커로 줄을 세우고, 판단은 부르지 않아요.
    let out = engine
        .ask("메일 탭 401 어디부터 봐요?", &[], engine.settings.show)
        .await;
    assert_eq!(out["cards"][0]["path"], "docs/메일/03_로그인.md");
    assert_eq!(
        (&out["shown"], &out["best_p"], &out["low_confidence"]),
        (&json!(true), &json!(0.95), &json!(false))
    );
    assert_eq!(reranker.calls().len(), 1);
    assert!(gater.calls().is_empty());

    // 엔진에 넣은 판단, 받아쓰기, 장부 모델이 그대로 불려요.
    let context = vec!["민준: 점심 뭐 먹어요".to_string()];
    let g = engine
        .services
        .gater
        .gate(&context, "지수: 그거 401 나면 어디 봐요?")
        .await
        .unwrap();
    assert_eq!((g.p_lookup, g.p_end), (0.83, Some(0.12)));
    assert_eq!(
        gater.calls(),
        [(context, "지수: 그거 401 나면 어디 봐요?".to_string())]
    );
    let heard = engine
        .services
        .transcriber
        .transcribe(b"RIFF")
        .await
        .unwrap();
    assert_eq!(
        (heard.text.as_str(), heard.confidence),
        ("메일 탭 401 나면 어디 봐요?", None)
    );
    assert_eq!(transcriber.calls(), [b"RIFF".to_vec()]);
    let ledger = engine
        .services
        .note_writer
        .ledger(
            "[s1u1] 나: 배포는 목요일",
            &[],
            tacie_engine::notes_kind::NotesKind::Meeting,
        )
        .await
        .unwrap();
    assert_eq!(ledger.text, r#"{"items": []}"#);
    assert_eq!(note_writer.calls(), ["[s1u1] 나: 배포는 목요일"]);
}

fn engine_pair(reranker: &Arc<FakeReranker>) -> (Engine, Arc<FakeEmbed>) {
    make_engine(index(), reranker, &Arc::new(FakeAnswerer::ok("")))
}

#[tokio::test]
async fn ask_falls_back_to_dense_order_when_jev_fails() {
    let message = format!("jev request failed: {}", "연결이 끊겼어요 ".repeat(40));
    let reranker = Arc::new(FakeReranker::fail(Kind::Network, &message));
    let (engine, _) = engine_pair(&reranker);
    let out = engine.ask("질문", &[], 0.8).await;
    assert_eq!(
        (&out["low_confidence"], &out["best_p"], &out["shown"]),
        (&json!(true), &Value::Null, &json!(true))
    );
    assert_eq!(out["cards"][0]["path"], "apps/api/src/mail/session.ts");
    assert_eq!(
        column(&out["cards"], "chunk"),
        [&json!(0), &json!(2), &json!(3)]
    );
    assert!(column(&out["cards"], "score").iter().all(|s| s.is_null()));
    assert_eq!(out["error_kind"], "network");
    assert_eq!(out["error"], message.chars().take(200).collect::<String>());
    assert_eq!(
        keys(&out),
        [
            "cards",
            "best_p",
            "shown",
            "low_confidence",
            "error",
            "error_kind",
            "timing_ms"
        ]
    );
    assert_eq!(keys(&out["timing_ms"]), ["search", "total"]);
}

#[tokio::test]
async fn ask_on_empty_index_makes_no_jev_call() {
    let reranker = Arc::new(FakeReranker::all(0.9));
    let (engine, embed) = engine_pair(&reranker);
    assert!(!engine.index_is_empty());

    // 이 사람이 볼 수 있는 조각이 하나도 없어요.
    let mut masked = index();
    masked.allowed = vec![false; masked.chunks.len()];
    engine.index.store(Arc::new(masked));
    // 동기화 전이거나 지운 색인: 조각이 없어요.
    let empty = index_of(&json!([]), &json!({}));
    for ix in [None, Some(empty)] {
        if let Some(ix) = ix {
            engine.index.store(Arc::new(ix));
        }
        assert!(engine.index_is_empty());
        let mut out = engine.ask("포트는요?", &[], 0.8).await;
        assert_eq!(keys(&out["timing_ms"]), ["search", "total"]);
        out.as_object_mut().unwrap().remove("timing_ms");
        assert_eq!(
            out,
            json!({"cards": [], "best_p": null, "shown": false, "low_confidence": false})
        );
        assert_eq!(keys(&out), ["cards", "best_p", "shown", "low_confidence"]);
    }
    assert!(reranker.calls().is_empty());
    assert_eq!(embed.calls(), 0);
}

/// 임베딩이 실패하면 카드 없이 이유를 실어요. 리랭커는 부르지 않아요.
#[tokio::test]
async fn ask_reports_a_failed_search() {
    let reranker = Arc::new(FakeReranker::all(0.9));
    let (mut engine, _) = engine_pair(&reranker);
    engine.embedder = Arc::new(FakeEmbed {
        fail: true,
        ..Default::default()
    });
    let mut out = engine.ask("포트는요?", &[], 0.8).await;
    assert_eq!(keys(&out["timing_ms"]), ["search", "total"]);
    out.as_object_mut().unwrap().remove("timing_ms");
    assert_eq!(
        out,
        json!({"cards": [], "best_p": null, "shown": false, "low_confidence": true,
               "error": "search failed: onnx run failed", "error_kind": "down"})
    );
    assert!(reranker.calls().is_empty());
}

#[tokio::test]
async fn compose_answer_returns_cited_chunk_ids_or_hides() {
    let reranker = Arc::new(FakeReranker::all(0.9));
    let answerer = Arc::new(FakeAnswerer::ok("둘째를 봐요. [2]"));
    let (engine, _) = make_engine(index(), &reranker, &answerer);
    let context: Vec<String> = ["a", "b", "c", "d"].map(String::from).to_vec();
    let out = engine.compose_answer("질문", &context, &[3, 0, 2]).await;
    assert_eq!(
        out,
        json!({"answer": "둘째를 봐요. [2]", "ms": 1.0, "cost": 0.0004, "cited": [0], "dropped": null})
    );
    assert_eq!(keys(&out), ["answer", "ms", "cost", "cited", "dropped"]);
    // 답 모델에는 카드 조각 전체의 본문(머리 줄 빼고)이 가고, 앞 대화는 마지막 세 줄만 가요.
    let (question, ctx, passages) = answerer.calls().remove(0);
    assert_eq!(
        (question.as_str(), ctx),
        ("질문", vec!["b".to_string(), "c".into(), "d".into()])
    );
    assert_eq!(
        passages,
        [
            json!({"path": "apps/web/src/mail/Tab.tsx", "line_start": 1, "snippet": "export const Tab = 1;"}),
            json!({"path": "apps/api/src/mail/session.ts", "line_start": 41,
                   "snippet": "export function createSession() {\n  return 1;\n}"}),
            json!({"path": "docs/메일/03_로그인.md", "line_start": 10, "snippet": "## 401이 날 때\n기동 순서를 확인해요."}),
        ]
    );

    let (hidden, _) = make_engine(index(), &reranker, &Arc::new(FakeAnswerer::ok("번호 없음")));
    let out = hidden.compose_answer("질문", &[], &[3, 0]).await;
    assert_eq!(
        (&out["answer"], &out["cited"], &out["dropped"]),
        (&Value::Null, &json!([]), &json!("citation"))
    );

    let (failing, _) = make_engine(
        index(),
        &reranker,
        &Arc::new(FakeAnswerer::fail(Kind::Credit, "answer 402: no credit")),
    );
    let out = failing.compose_answer("질문", &[], &[0]).await;
    assert_eq!(
        out,
        json!({"answer": null, "cited": [], "dropped": null, "error": "answer 402: no credit", "error_kind": "credit"})
    );
    assert_eq!(
        keys(&out),
        ["answer", "cited", "dropped", "error", "error_kind"]
    );
}

/// `/answer`는 거를 때 읽은 색인을 넘겨요: 그사이 갱신이 다른 색인을 넣어도 걸러 둔 조각 id로 옛 색인의 글을 써요.
#[tokio::test]
async fn answer_uses_the_index_it_was_given() {
    let reranker = Arc::new(FakeReranker::all(0.9));
    let answerer = Arc::new(FakeAnswerer::ok("둘째를 봐요. [1]"));
    let (engine, _) = make_engine(index(), &reranker, &answerer);
    let old = engine.index.load_full();
    engine
        .index
        .store(Arc::new(index_of(&json!([]), &json!({}))));
    let out = engine.compose_answer_from(&old, "질문", &[], &[2]).await;
    assert_eq!(out["cited"], json!([2]));
    assert_eq!(
        answerer.calls()[0].2,
        [json!({"path": "docs/메일/03_로그인.md", "line_start": 10,
                "snippet": "## 401이 날 때\n기동 순서를 확인해요."})]
    );
}

#[tokio::test]
async fn passages_are_cut_at_900_and_answer_bodies_at_1500() {
    let long = "가".repeat(2000);
    let one =
        json!([{"src": "docs", "path": "a.md", "line": 1, "text": format!("doc: a.md\n{long}")}]);
    let reranker = Arc::new(FakeReranker::all(0.9));
    let answerer = Arc::new(FakeAnswerer::ok("[1]"));
    let (engine, _) = make_engine(index_of(&one, &json!({})), &reranker, &answerer);
    engine.ask("질문", &[], 0.8).await;
    let passage = &reranker.calls()[0].2[0];
    assert_eq!(
        *passage,
        format!("doc: a.md\n{long}")
            .chars()
            .take(900)
            .collect::<String>()
    );
    engine.compose_answer("질문", &[], &[0]).await;
    assert_eq!(answerer.calls()[0].2[0]["snippet"], "가".repeat(1500));
}

/// 손으로 쓴 체크아웃이에요(git을 띄우지 않아요): `.git` 폴더에 HEAD, objects, refs와 origin이 `url`인 config예요.
fn checkout_with_origin(dir: &Path, url: &str) {
    let git = dir.join(".git");
    for sub in ["objects", "refs/heads"] {
        std::fs::create_dir_all(git.join(sub)).unwrap();
    }
    std::fs::write(git.join("HEAD"), "ref: refs/heads/main\n").unwrap();
    set_origin(&git, url);
}

/// `git_dir/config`의 origin 주소예요.
fn set_origin(git_dir: &Path, url: &str) {
    std::fs::write(
        git_dir.join("config"),
        format!(
            "[core]\n\trepositoryformatversion = 0\n\tbare = false\n[remote \"origin\"]\n\turl = {url}\n"
        ),
    )
    .unwrap();
}

/// `main`의 워크트리예요: `.git`은 `gitdir:` 파일이고 config는 본 저장소의 것이에요.
fn worktree_of(main: &Path, dir: &Path) {
    let admin = main.join(".git/worktrees/wt");
    std::fs::create_dir_all(&admin).unwrap();
    std::fs::write(admin.join("HEAD"), "ref: refs/heads/wt\n").unwrap();
    std::fs::write(admin.join("commondir"), "../..\n").unwrap();
    std::fs::create_dir_all(dir).unwrap();
    std::fs::write(
        admin.join("gitdir"),
        format!("{}\n", dir.join(".git").display()),
    )
    .unwrap();
    std::fs::write(dir.join(".git"), format!("gitdir: {}\n", admin.display())).unwrap();
}

#[tokio::test]
async fn local_path_only_for_the_checked_out_repo() {
    let tmp = tempfile::tempdir().unwrap();
    let checkout = tmp.path().join("checkout");
    checkout_with_origin(&checkout, "https://github.com/o/old.git");
    let reranker = Arc::new(FakeReranker::all(0.9));
    // 후보는 0과 2만이에요.
    let index_for = |repo: &str| {
        let mut ix = index_of(&chunks(), &json!({"repo": repo, "commit": "abc123"}));
        ix.allowed = vec![true, false, true, false];
        ix
    };
    let make = |repo: &str, dir: PathBuf| {
        make_engine(index_for(repo), &reranker, &Arc::new(FakeAnswerer::ok("")))
            .0
            .with_repo_dir(Some(dir))
    };
    async fn local_paths(engine: &Engine) -> Vec<Value> {
        let out = engine.ask("세션 어디서 만들어요?", &[], 0.8).await;
        out["cards"]
            .as_array()
            .unwrap()
            .iter()
            .map(|c| c["local_path"].clone())
            .collect()
    }
    let at = |p: &str| json!(checkout.join(p).to_string_lossy());

    assert_eq!(checkout_repo(&checkout).as_deref(), Some("o/old"));
    let engine = make("o/old", checkout.clone());
    assert_eq!(
        local_paths(&engine).await,
        [
            at("apps/api/src/mail/session.ts"),
            at("docs/메일/03_로그인.md")
        ]
    );
    engine.index.store(Arc::new(index_for("o/new"))); // 갱신이 다른 저장소의 색인으로 바꿨어요
    assert_eq!(local_paths(&engine).await, [Value::Null, Value::Null]);
    assert_eq!(
        local_paths(&make("o/new", checkout.clone())).await,
        [Value::Null, Value::Null]
    );
    set_origin(&checkout.join(".git"), "git@github.com:O/Old.git");
    assert_eq!(checkout_repo(&checkout).as_deref(), Some("O/Old"));
    // 워크트리(`.git`이 `gitdir:` 파일)도 본 저장소의 origin이에요
    let worktree = tmp.path().join("worktree");
    worktree_of(&checkout, &worktree);
    assert_eq!(checkout_repo(&worktree).as_deref(), Some("O/Old"));
    assert!(!local_paths(&make("o/old", checkout.clone())).await[0].is_null());
    // 다른 remote 뒤, 주석 줄과 따옴표, 줄 끝 주석이 있어도 origin의 첫 url이에요
    let config = checkout.join(".git/config");
    std::fs::write(
        &config,
        "[core]\n\tbare = false\n[remote \"upstream\"]\n\turl = https://github.com/up/stream.git\n\
         [Remote \"origin\"]\n\t# url = https://github.com/no/hash.git\n\t; url = https://github.com/no/semi.git\n\
         \tfetch = +refs/heads/*:refs/remotes/origin/*\n\tURL = \"https://github.com/q/r;x.git\" ; first\n\
         \turl = https://github.com/no/second.git\n[branch \"main\"]\n\tremote = origin\n",
    )
    .unwrap();
    assert_eq!(checkout_repo(&checkout).as_deref(), Some("q/r;x"));
    assert_eq!(checkout_repo(&worktree).as_deref(), Some("q/r;x"));
    // 주석 뒤의 값, 앞뒤 공백
    std::fs::write(
        &config,
        "[remote \"origin\"]\n  url =   git@github.com:a/b.git   # mine\n",
    )
    .unwrap();
    assert_eq!(checkout_repo(&checkout).as_deref(), Some("a/b"));
    // origin이 없거나 url이 없으면 None이에요
    std::fs::write(
        &config,
        "[remote \"upstream\"]\n\turl = https://github.com/u/p.git\n",
    )
    .unwrap();
    assert_eq!(checkout_repo(&checkout), None);
    std::fs::write(&config, "[remote \"origin\"]\n\tfetch = x\n").unwrap();
    assert_eq!(checkout_repo(&checkout), None);
    // 상대 경로의 `gitdir:`이고 commondir이 없는 워크트리(config가 gitdir에 있어요)
    let relative = tmp.path().join("relative");
    let admin = tmp.path().join("admin");
    std::fs::create_dir_all(&relative).unwrap();
    std::fs::create_dir_all(&admin).unwrap();
    std::fs::write(relative.join(".git"), "gitdir: ../admin\n").unwrap();
    set_origin(&admin, "https://github.com/rel/ative.git");
    assert_eq!(checkout_repo(&relative).as_deref(), Some("rel/ative"));
    let missing = tmp.path().join("missing");
    assert_eq!(checkout_repo(&missing), None);
    assert_eq!(
        local_paths(&make("o/old", missing)).await,
        [Value::Null, Value::Null]
    );
}

/// (모델이 쓴 줄, 답, 근거 번호, 버린 이유)
type Case = (String, Option<String>, Vec<usize>, Option<&'static str>);

/// `check_answer`의 규칙이에요. 기대값은 예전 Python 엔진이 만든 고정 값이에요(2026-10-26).
#[test]
fn check_answer_rules() {
    let long = "가".repeat(201) + " [1]";
    let fits = "가".repeat(196) + " [1]"; // 200자, 바이트로는 592예요: 글자 수로 세요
    let mut table: Vec<Case> = vec![
        // test_check_answer_rules
        (
            "dev:doctor부터 돌려요. [1]".into(),
            Some("dev:doctor부터 돌려요. [1]".into()),
            vec![1],
            None,
        ),
        (
            "둘 다 봐요. [1][3]".into(),
            Some("둘 다 봐요. [1][3]".into()),
            vec![1, 3],
            None,
        ),
        ("번호 없는 답이에요.".into(), None, vec![], Some("citation")),
        ("범위 밖이에요. [4]".into(), None, vec![], Some("citation")),
        ("영 번이에요. [0]".into(), None, vec![], Some("citation")),
        (long, None, vec![], Some("length")),
        (fits.clone(), Some(fits), vec![1], None),
        // test_no_answer_sentence_passes_through
        (
            "찾은 자료에는 답이 없어요.".into(),
            Some("찾은 자료에는 답이 없어요.".into()),
            vec![],
            None,
        ),
        (
            "찾은 자료에는 답이 없어요. [1]".into(),
            Some("찾은 자료에는 답이 없어요. [1]".into()),
            vec![],
            None,
        ),
        // test_grouped_citations_are_split
        (
            "설정은 여기 있어요. [1, 3]".into(),
            Some("설정은 여기 있어요. [1][3]".into()),
            vec![1, 3],
            None,
        ),
        // test_answer_drops_a_leading_source_phrase
        (
            "자료 1에 따르면 서명 기본 위치는 인용 위예요. [1][3]".into(),
            Some("서명 기본 위치는 인용 위예요. [1][3]".into()),
            vec![1, 3],
            None,
        ),
        (
            "자료에 따르면, 영문 표기는 Asgarrd예요. [1]".into(),
            Some("영문 표기는 Asgarrd예요. [1]".into()),
            vec![1],
            None,
        ),
    ];
    // test_source_phrase_variants_are_dropped_and_their_numbers_kept: 문구의 번호는 끝에 표시로 돌아와요.
    for (phrase, marks, cited) in [
        ("자료 [1]에 따르면", "[1]", vec![1]),
        ("1번 자료에 따르면", "[1]", vec![1]),
        ("자료 1, 2에 따르면", "[1][2]", vec![1, 2]),
        ("자료 [1][2]에 따르면", "[1][2]", vec![1, 2]),
        ("자료(1)와 2에 따르면", "[1][2]", vec![1, 2]),
        ("자료 1·2에 따르면", "[1][2]", vec![1, 2]),
    ] {
        table.push((
            format!("{phrase} 링크는 7일 동안 살아 있어요."),
            Some(format!("링크는 7일 동안 살아 있어요. {marks}")),
            cited,
            None,
        ));
        // 이미 단 번호는 다시 붙이지 않아요.
        table.push((
            format!("{phrase} 7일이에요. [2][1]"),
            Some("7일이에요. [2][1]".into()),
            vec![2, 1],
            None,
        ));
    }
    table.extend([
        // 범위 밖이면 그래도 숨겨요.
        (
            "자료 [4]에 따르면 7일이에요.".into(),
            None,
            vec![],
            Some("citation"),
        ),
        // test_code_indices_are_not_citations
        (
            "`argv[0]`를 봐요. [1]".into(),
            Some("`argv[0]`를 봐요. [1]".into()),
            vec![1],
            None,
        ),
        (
            "`items[2]`에 있어요. [1]".into(),
            Some("`items[2]`에 있어요. [1]".into()),
            vec![1],
            None,
        ),
        (
            "`a[1, 2]`예요. [3]".into(),
            Some("`a[1, 2]`예요. [3]".into()),
            vec![3],
            None,
        ),
        // test_no_answer_sentence_after_junk_still_counts_as_no_answer
        (
            "```\n```\n\n<reasoning_effort>8</reasoning_effort>\n\n찾은 자료에는 답이 없어요."
                .into(),
            Some("찾은 자료에는 답이 없어요.".into()),
            vec![],
            None,
        ),
        (
            "\"찾은 자료에는 답이 없어요.\"".into(),
            Some("찾은 자료에는 답이 없어요.".into()),
            vec![],
            None,
        ),
        (
            "설정은 a.ts에 있어요 [1]. 기본값은 찾은 자료에는 답이 없어요.".into(),
            Some("설정은 a.ts에 있어요 [1]. 기본값은 찾은 자료에는 답이 없어요.".into()),
            vec![1],
            None,
        ),
        // 2026-11-04: 질문을 되뇐 앞머리, 자료를 번호로 가리키는 답
        (
            "회의 질문 \"기존 DB는 안 건드려서 상관없나?\"에 대해 자료 1에 따르면 스키마 변경이 없어요. [1]".into(),
            Some("스키마 변경이 없어요. [1]".into()),
            vec![1],
            None,
        ),
        (
            "질문 「재시도는?」에 대해서, 세 번까지예요. [2]".into(),
            Some("세 번까지예요. [2]".into()),
            vec![2],
            None,
        ),
        (
            "자료 1에는 메일함을 계정별로 나눈다고 적혀 있어요. [1]".into(),
            None,
            vec![],
            Some("source"),
        ),
        (
            "2번 자료를 보면 세 번이에요. [2]".into(),
            None,
            vec![],
            Some("source"),
        ),
        (
            "`자료1` 함수에서 세 번까지 해요. [1]".into(),
            Some("`자료1` 함수에서 세 번까지 해요. [1]".into()),
            vec![1],
            None,
        ),
        (
            "같은 질문 \"x\"에 대해 팀에서 세 번으로 정했어요. [1]".into(),
            Some("같은 질문 \"x\"에 대해 팀에서 세 번으로 정했어요. [1]".into()),
            vec![1],
            None,
        ),
    ]);
    for (text, answer, cited, dropped) in table {
        assert_eq!(check_answer(&text, 3), (answer, cited, dropped), "{text:?}");
    }
}

/// 온보딩 연습(`sample: true`)은 예시 색인의 조각 전부를 후보로 써서 임베딩(모델)을 부르지 않아요.
#[tokio::test]
async fn a_sample_question_searches_the_whole_sample_index_without_embedding() {
    use tacie_engine::sample::{Question, SAMPLE_BASE, Sample};
    let reranker = Arc::new(FakeReranker::ok(&[0.1, 0.2, 0.95, 0.3]));
    let answerer = Arc::new(FakeAnswerer::ok("x"));
    let mine = index_of(
        &json!([{"src": "docs", "path": "mine.md", "line": 1, "text": "file: mine.md\n내 자료"}]),
        &json!({}),
    );
    let (engine, embed) = make_engine(mine, &reranker, &answerer);
    let engine = engine.with_sample(Some(Sample {
        index: Arc::new(index()),
        questions: vec![Question {
            text: "로그인은 어디서 해요?".into(),
            purpose: None,
        }],
    }));
    let out = engine
        .ask_in(true, "아무렇게나 받아쓴 질문", &[], 0.8)
        .await;
    assert_eq!(out["sample"], json!(true));
    assert_eq!(out["shown"], json!(true));
    // 리랭커가 0.95를 준 조각 2가 먼저예요. id에는 SAMPLE_BASE가 더해져요.
    assert_eq!(out["cards"][0]["chunk"], json!(SAMPLE_BASE + 2));
    assert!(
        column(&out["cards"], "path")
            .iter()
            .all(|p| p.as_str().unwrap() != "mine.md")
    );
    assert_eq!(
        embed.calls.load(std::sync::atomic::Ordering::SeqCst),
        0,
        "no embedding for the sample"
    );

    // 내 자료 요청은 예시 색인을 보지 않아요.
    let mine = engine.ask("로그인은 어디서 해요?", &[], 0.0).await;
    assert!(mine.get("sample").is_none());
    assert!(
        column(&mine["cards"], "path")
            .iter()
            .all(|p| p.as_str().unwrap() == "mine.md")
    );
}

fn meeting_chunks() -> Value {
    let mut all = chunks().as_array().unwrap().clone();
    for id in ["a", "b"] {
        all.push(
            json!({"src": "meeting", "path": format!("meeting/{id}"), "line": 1,
                        "text": format!("meeting: {id}\nsummary: 세션 이야기")}),
        );
    }
    Value::Array(all)
}

#[tokio::test]
async fn ask_scoped_keeps_only_the_meetings_in_scope() {
    let reranker = Arc::new(FakeReranker::ok(&[0.9, 0.8, 0.7, 0.6, 0.5, 0.4]));
    let ix = index_of(&meeting_chunks(), &json!({"repo": "acme/widgets"}));
    let (engine, _) = make_engine(ix, &reranker, &Arc::new(FakeAnswerer::ok("")));
    let out = engine
        .ask_scoped(
            "세션 어디서 만들어요?",
            &[],
            &Scope::Meetings(vec!["a".to_string()]),
            engine.settings.show,
        )
        .await;
    let cards = out["cards"].as_array().unwrap();
    assert!(!cards.is_empty());
    assert!(cards.iter().all(|c| c["path"] == "meeting/a"), "{out}");
    // 리랭커에는 범위 안 조각만 가요.
    assert_eq!(reranker.calls()[0].2.len(), 1);
}

#[tokio::test]
async fn ask_scoped_finds_a_meeting_outside_the_global_top_fifty() {
    // 회의 조각보다 점수가 높거나 같은(먼저 오는) 조각이 60개예요: 전체 위 50개에는 회의가 들지 않아요.
    let mut all: Vec<Value> = (0..60)
        .map(|i| json!({"src": "code", "path": format!("src/f{i}.rs"), "line": 1, "text": format!("file: src/f{i}.rs\nfn f{i}() {{}}")}))
        .collect();
    all.push(json!({"src": "meeting", "path": "meeting/a", "line": 1, "text": "meeting: a\nsummary: 세션 이야기"}));
    let ix = index_of(&Value::Array(all), &json!({"repo": "acme/widgets"}));
    let reranker = Arc::new(FakeReranker::all(0.9));
    let (engine, _) = make_engine(ix, &reranker, &Arc::new(FakeAnswerer::ok("")));
    let out = engine
        .ask_scoped(
            "세션 어디서 만들어요?",
            &[],
            &Scope::Meetings(vec!["a".to_string()]),
            engine.settings.show,
        )
        .await;
    let cards = out["cards"].as_array().unwrap();
    assert_eq!(cards.len(), 1, "{out}");
    assert_eq!(cards[0]["path"], "meeting/a");
    assert_eq!(reranker.calls()[0].2.len(), 1);
}

#[tokio::test]
async fn ask_scoped_all_matches_ask() {
    let reranker = Arc::new(FakeReranker::ok(&[0.1, 0.2, 0.95, 0.3]));
    let (engine, _) = make_engine(index(), &reranker, &Arc::new(FakeAnswerer::ok("")));
    let plain = engine.ask("메일 탭 401", &[], engine.settings.show).await;
    let scoped = engine
        .ask_scoped("메일 탭 401", &[], &Scope::All, engine.settings.show)
        .await;
    assert_eq!(
        column(&plain["cards"], "chunk"),
        column(&scoped["cards"], "chunk")
    );
    assert_eq!(plain["best_p"], scoped["best_p"]);
}
