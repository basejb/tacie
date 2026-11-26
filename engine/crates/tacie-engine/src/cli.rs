//! Dev commands for evaluation. Reads JSON from stdin in one go and writes one line of JSON to stdout.

use std::collections::{HashMap, HashSet};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Instant;

use anyhow::Context;
use serde::{Deserialize, Serialize};
use tacie_embed::voice::{VoiceEmbed, VoiceEmbedder};
use tacie_embed::{Device, Embed, OnnxEmbedder, PASSAGE_TOKENS};
use tacie_index::Index;
use tacie_sources::github::content::{Entry, Snapshot, Tree, blob_sha};

use tacie_engine::sample::{BuildQuestion, Question as SampleQuestion, STRICT_TOP, answer_rank};

use crate::{CandArgs, DeviceArg, EmbedArgs, IndexFolderArgs, VoiceEmbedArgs};

/// `--model-dir`(또는 `TACIE_MODEL_DIR`)가 없을 때 쓰는 폴더예요. 세 명령이 모두 이걸 써요.
pub fn model_dir(arg: Option<PathBuf>) -> PathBuf {
    arg.unwrap_or_else(|| {
        let home = std::env::var_os("HOME").unwrap_or_default();
        PathBuf::from(home).join("Library/Application Support/Tacie/models/bge-m3-onnx")
    })
}

/// 화자 임베딩 모델(docs/04 4절)을 열어요. 없거나 열리지 않으면 `voice: off (…)`를 stderr에 쓰고 None이에요:
/// 엔진은 목소리 이름 없이 그대로 떠요.
pub fn load_voice(arg: Option<PathBuf>, diar: Option<PathBuf>) -> Option<Arc<dyn VoiceEmbed>> {
    let path = arg.unwrap_or_else(|| {
        let home = std::env::var_os("HOME").unwrap_or_default();
        PathBuf::from(home)
            .join("Library/Application Support/Tacie/models/eres2net-speaker/model.onnx")
    });
    if !path.is_file() {
        eprintln!("voice: off (missing {})", path.display());
        return None;
    }
    let mut voice = match VoiceEmbedder::load(&path) {
        Ok(v) => v,
        Err(e) => {
            eprintln!("voice: off ({})", format!("{e:#}").replace('\n', " "));
            return None;
        }
    };
    // 화자 나누기 모델(CAM++ 파인튜닝)이 있으면 화자를 그 임베딩으로 묶고, ERes2Net은 기억한 참석자와 견줄 때만 써요.
    // 없거나 열리지 않으면 지금처럼 ERes2Net 하나로 묶어요(docs/04 4절 "화자 나누기와 참석자 확인").
    // `off`: the app turned the speaker-grouping model off (the server's switch, docs/04 4절 "되돌리기").
    if diar.as_deref() == Some(std::path::Path::new("off")) {
        eprintln!("voice: eres2net only (turned off)");
        return Some(Arc::new(voice));
    }
    let diar = diar.unwrap_or_else(|| {
        let home = std::env::var_os("HOME").unwrap_or_default();
        PathBuf::from(home)
            .join("Library/Application Support/Tacie/models/campp-ft-speaker/model.onnx")
    });
    if !diar.is_file() {
        eprintln!("voice: eres2net only (missing {})", diar.display());
        return Some(Arc::new(voice));
    }
    if let Err(e) = voice.add_diar(&diar) {
        eprintln!(
            "voice: eres2net only ({})",
            format!("{e:#}").replace('\n', " ")
        );
    }
    Some(Arc::new(voice))
}

/// 모델 파일(그래프, 가중치, 토크나이저) 중 하나라도 없으면 `model: missing <dir>`를 stderr에 쓰고
/// 종료 코드 3으로 끝나요. 앱은 이 코드로 "모델을 받아야 해요"를 보여 줄 수 있어요.
/// 가중치 대신 받은 fp16 가중치(`model.onnx_data.f16`)가 있으면 먼저 공식 fp32로 풀어요(한 번, 몇 초).
/// 풀다 실패하면 `model: unpack failed …`를 쓰고 같은 코드 3으로 끝나요.
pub fn require_model(dir: PathBuf) -> PathBuf {
    find_model(dir).unwrap_or_else(|| std::process::exit(3))
}

/// `require_model`처럼 찾고 풀지만, 없으면 끝내지 않고 None이에요(같은 `model: …` 줄은 남겨요). 서버는 모델 없이도
/// 떠서 예시 질문에 답하고, 임베딩이 필요한 요청만 `model`로 실패해요(docs/11 12절).
pub fn find_model(dir: PathBuf) -> Option<PathBuf> {
    const FILES: [&str; 3] = ["model.onnx", tacie_embed::WEIGHTS, "tokenizer.json"];
    if dir.join(tacie_embed::PACKED_WEIGHTS).is_file() && !dir.join(tacie_embed::WEIGHTS).exists() {
        eprintln!("model: unpacking weights in {}", dir.display());
        let t = Instant::now();
        match tacie_embed::unpack_weights(&dir) {
            Ok(_) => eprintln!("model: unpacked in {:.1} s", t.elapsed().as_secs_f64()),
            Err(e) => {
                eprintln!(
                    "model: unpack failed {}",
                    format!("{e:#}").replace('\n', " ")
                );
                return None;
            }
        }
    }
    if !FILES.iter().all(|f| dir.join(f).is_file()) {
        eprintln!("model: missing {}", dir.display());
        return None;
    }
    Some(dir)
}

/// 폴더 하나로 색인을 만들어요(온보딩 예시 자료, docs/11 12절). 폴더를 커밋 하나처럼 `Snapshot`으로 만들어
/// (`folder_snapshot`) 저장소를 자르는 `repo::chunks`를 그대로 써요: git 없이 같은 자르기, 같은 색인 형식이에요.
/// `--questions`가 있으면 예시 질문(`questions.json`, `[{text, use}]`)도 같이 둬요.
pub fn run_index_folder(args: IndexFolderArgs) -> anyhow::Result<()> {
    let model = require_model(model_dir(args.model_dir));
    let embedder = OnnxEmbedder::load(&model, Device::Cpu)?;
    let snap =
        folder_snapshot(&args.folder).with_context(|| format!("read {}", args.folder.display()))?;
    let chunks = tacie_sources::repo::chunks(&snap, None, &[], None)
        .map_err(|e| anyhow::anyhow!("{e}"))
        .with_context(|| format!("cut {}", args.folder.display()))?;
    // 예시 색인은 조각 전부를 후보로 써요(`Sample::open`도 같은 걸 봐요).
    anyhow::ensure!(
        chunks.len() <= tacie_index::CAP,
        "{} chunks: a sample index holds at most {}",
        chunks.len(),
        tacie_index::CAP
    );
    let mut meta = serde_json::Map::new();
    meta.insert("repo".into(), "".into());
    meta.insert("commit".into(), "sample".into());
    meta.insert("scope".into(), "sample".into());
    meta.insert(
        "embed".into(),
        serde_json::json!({"model": tacie_index::MODEL, "tokens": PASSAGE_TOKENS}),
    );
    let n = tacie_index::write_index(
        &args.out,
        &chunks,
        &mut |texts| embedder.embed(texts, Some(PASSAGE_TOKENS)),
        &meta,
        None,
        None,
    )?;
    if let Some(q) = &args.questions {
        // 예시 질문은 텍스트와 쓰임만 둬요: 예시 색인은 조각 전부가 후보라 질문 벡터가 필요 없어요.
        #[derive(Deserialize)]
        struct File {
            questions: Vec<BuildQuestion>,
        }
        let file: File = serde_json::from_str(
            &std::fs::read_to_string(q).with_context(|| format!("read {}", q.display()))?,
        )
        .with_context(|| format!("parse {}", q.display()))?;
        strict_check(&args.out, &file.questions, &embedder)?;
        let questions: Vec<&SampleQuestion> = file.questions.iter().map(|b| &b.question).collect();
        std::fs::write(
            args.out.join("questions.json"),
            serde_json::to_vec(&questions)?,
        )?;
    }
    eprintln!(
        "index-folder: {} chunks ({n} embedded) -> {}",
        chunks.len(),
        args.out.display()
    );
    Ok(())
}

/// 예시 질문마다 검색 모델(bge-m3)로 찾은 위 `STRICT_TOP`조각 안에 정답 파일(`answer`)이 있는지 봐요. 질문마다
/// 위 5조각의 경로와 정답이 처음 나온 순위를 stderr에 쓰고, 하나라도 빠지면 실패해요. 연습에서는 리랭커가 조각 전부를
/// 보지만, 무료로 다시 돌릴 수 있는 이 확인을 바닥으로 둬요. `answer`가 없는 질문은 건너뛰어요.
fn strict_check(
    out: &Path,
    questions: &[BuildQuestion],
    embedder: &OnnxEmbedder,
) -> anyhow::Result<()> {
    let index = Index::open(out, tacie_index::Mask::None, &HashSet::new())?;
    let mut missed = Vec::new();
    for b in questions.iter().filter(|b| !b.answer.is_empty()) {
        let vec = embedder.embed(&[b.question.text.as_str()], None)?.remove(0);
        let ranked: Vec<&str> = index
            .dense(&vec, index.chunks.len())
            .into_iter()
            .map(|i| index.chunks[i].path.as_str())
            .collect();
        let rank = answer_rank(&ranked, &b.answer);
        let ok = rank.is_some_and(|r| r <= STRICT_TOP);
        eprintln!(
            "strict {} {:?}: answer at {} | top {}",
            if ok { "ok  " } else { "MISS" },
            b.question.text,
            rank.map_or("-".to_string(), |r| r.to_string()),
            ranked[..ranked.len().min(5)].join(", ")
        );
        if !ok {
            missed.push(b.question.text.clone());
        }
    }
    anyhow::ensure!(
        missed.is_empty(),
        "answer files not in the top {STRICT_TOP} for: {}",
        missed.join(" / ")
    );
    Ok(())
}

/// 폴더 아래 보통 파일로 만든 `Snapshot`이에요: 모두 모드 100644, 경로 바이트 순서이고(`git ls-files`와 같아요),
/// 심볼릭 링크와 `.git` 폴더는 빼요. 자르개가 읽는 파일(`repo::is_wanted`)과 기본 그래프
/// `graft/.graph/wiring.json`(있으면)만 읽고 트리에 넣어요: 다른 파일(큰 바이너리 같은)은 읽지 않아요.
fn folder_snapshot(root: &Path) -> anyhow::Result<Snapshot> {
    let mut files: Vec<(String, Vec<u8>)> = Vec::new();
    let mut stack = vec![root.to_path_buf()];
    while let Some(dir) = stack.pop() {
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        for e in entries.flatten() {
            let Ok(kind) = e.file_type() else { continue };
            let path = e.path();
            if kind.is_dir() && e.file_name() != ".git" {
                stack.push(path);
            } else if kind.is_file()
                && let Ok(rel) = path.strip_prefix(root)
            {
                let rel = rel.to_string_lossy().replace('\\', "/");
                if !tacie_sources::repo::is_wanted(&rel) && rel != tacie_sources::repo::GRAPH_PATH {
                    continue;
                }
                let bytes =
                    std::fs::read(&path).with_context(|| format!("read {}", path.display()))?;
                files.push((rel, bytes));
            }
        }
    }
    files.sort_by(|a, b| a.0.as_bytes().cmp(b.0.as_bytes()));
    let entries = files
        .iter()
        .map(|(path, bytes)| Entry {
            path: path.clone(),
            mode: "100644".to_string(),
            sha: blob_sha(bytes),
        })
        .collect();
    let contents: HashMap<String, Vec<u8>> = files.into_iter().collect();
    let graph = contents.get(tacie_sources::repo::GRAPH_PATH).cloned();
    Ok(Snapshot {
        tree: Tree {
            commit: String::new(),
            entries,
        },
        contents,
        graph,
    })
}

#[derive(Deserialize)]
struct Question {
    id: String,
    text: String,
    #[serde(default)]
    context: Vec<String>,
    /// 없으면 `"나: " + text`예요(eval/slack_eval.py, eval/github_eval.py와 같아요).
    line: Option<String>,
}

#[derive(Serialize)]
struct Candidates {
    id: String,
    cands: Vec<usize>,
}

/// stdin `[{"id","text","context","line"}]` → stdout `[{"id","cands"}]`, 입력 순서 그대로예요.
/// 보는 사람(viewer)은 비워요: 평가 세트에는 비공개 슬랙 조각이 없어요.
pub fn run_candidates(args: CandArgs) -> anyhow::Result<()> {
    let model = require_model(model_dir(args.model_dir));
    let questions: Vec<Question> =
        serde_json::from_str(&read_stdin()?).context("stdin is not a JSON array of questions")?;

    let t = Instant::now();
    let index = Index::open(&args.index_dir, args.mask, &HashSet::new())
        .with_context(|| format!("open index {}", args.index_dir.display()))?;
    let prefer = match args.device {
        DeviceArg::Coreml => Device::CoreMl,
        DeviceArg::Cpu => Device::Cpu,
    };
    let embedder = OnnxEmbedder::load(&model, prefer)?;
    let load_s = t.elapsed().as_secs_f64();

    let t = Instant::now();
    let mut out = Vec::with_capacity(questions.len());
    for q in questions {
        let line = q.line.unwrap_or_else(|| format!("나: {}", q.text));
        let cands = index
            .candidates(&embedder, &q.text, &q.context, &line)
            .with_context(|| format!("question {}", q.id))?;
        out.push(Candidates { id: q.id, cands });
    }
    eprintln!(
        "candidates: {} questions in {:.1} s (load {load_s:.1} s)",
        out.len(),
        t.elapsed().as_secs_f64()
    );
    write_stdout(&out)
}

/// 한 번에 넣는 글 수예요. 4개보다 크게 해도 빨라지지 않고 메모리만 커져요(docs/01).
const BATCH: usize = 4;

/// stdin `["글", ...]` → stdout `[[f32; 1024], ...]`. `--passage`면 색인 조각처럼 512토큰에서 잘라요.
pub fn run_embed(args: EmbedArgs) -> anyhow::Result<()> {
    let model = require_model(model_dir(args.model_dir));
    let texts: Vec<String> =
        serde_json::from_str(&read_stdin()?).context("stdin is not a JSON array of strings")?;
    let embedder = OnnxEmbedder::load(&model, Device::Cpu)?;
    let max_tokens = args.passage.then_some(PASSAGE_TOKENS);
    let mut out = Vec::with_capacity(texts.len());
    for batch in texts.chunks(BATCH) {
        let refs: Vec<&str> = batch.iter().map(String::as_str).collect();
        out.extend(embedder.embed(&refs, max_tokens)?);
    }
    write_stdout(&out)
}

/// 평가용: WAV마다 화자 임베딩을 한 줄(JSON 배열)로 써요. 모델이 없으면 `load_voice`처럼 넘어가지 않고 실패해요.
pub fn run_voice_embed(args: VoiceEmbedArgs) -> anyhow::Result<()> {
    let embedder = match args.diar_only {
        Some(d) => VoiceEmbedder::diar_only(&d)?,
        None => eres2net(args.voice_model)?,
    };
    for wav in &args.wavs {
        let bytes = std::fs::read(wav).with_context(|| format!("read {}", wav.display()))?;
        let print = embedder
            .embed(&bytes)
            .with_context(|| format!("embed {}", wav.display()))?;
        write_stdout(&print.map(|p| p.vector))?;
    }
    Ok(())
}

fn eres2net(model: Option<PathBuf>) -> anyhow::Result<VoiceEmbedder> {
    let path = model.unwrap_or_else(|| {
        let home = std::env::var_os("HOME").unwrap_or_default();
        PathBuf::from(home)
            .join("Library/Application Support/Tacie/models/eres2net-speaker/model.onnx")
    });
    anyhow::ensure!(path.is_file(), "missing {}", path.display());
    VoiceEmbedder::load(&path)
}

fn read_stdin() -> anyhow::Result<String> {
    let mut s = String::new();
    std::io::stdin()
        .read_to_string(&mut s)
        .context("read stdin")?;
    Ok(s)
}

fn write_stdout<T: Serialize>(value: &T) -> anyhow::Result<()> {
    let mut stdout = std::io::stdout().lock();
    serde_json::to_writer(&mut stdout, value)?;
    stdout.write_all(b"\n")?;
    stdout.flush()?;
    Ok(())
}

/// 시험용 용어 목록이에요: 파일(한 줄에 하나)이 있으면 그것을, 없으면 색인에서 뽑은 것을 써요.
fn terms_list(file: Option<&std::path::Path>, index: &Index) -> anyhow::Result<Vec<String>> {
    Ok(match file {
        Some(f) => std::fs::read_to_string(f)
            .with_context(|| format!("read {}", f.display()))?
            .lines()
            .map(str::trim)
            .filter(|l| !l.is_empty())
            .map(String::from)
            .collect(),
        None => tacie_engine::terms::extract(&index.chunks, &index.allowed),
    })
}

/// stdin: 받아쓴 글의 JSON 배열 → stdout `{"terms", "missing", "prompts", "fixed"}`. 회의가 쓰는 것과 같은 용어
/// 뽑기와 고치기예요(`terms`). 표기 파일은 읽기만 하고, 표기가 없는 용어와 그걸 물을 요청 글을 같이 내요.
pub fn run_fix_terms(args: crate::FixTermsArgs) -> anyhow::Result<()> {
    use tacie_engine::terms::{READINGS_BATCH, Readings, Terms};
    let texts: Vec<String> =
        serde_json::from_str(&read_stdin()?).context("stdin is not a JSON array of strings")?;
    let index = Index::open(&args.index_dir, args.mask, &HashSet::new())
        .with_context(|| format!("open index {}", args.index_dir.display()))?;
    let list = terms_list(args.terms_file.as_deref(), &index)?;
    let readings = Readings::load(&args.readings);
    let missing = readings.missing(&list);
    // 프롬프트 글이 없는 엔진(배포 빌드)은 빈 목록이에요.
    #[cfg(feature = "local-prompts")]
    let prompts: Vec<String> = missing
        .chunks(READINGS_BATCH)
        .map(tacie_engine::terms::readings_prompt)
        .collect();
    #[cfg(not(feature = "local-prompts"))]
    let prompts: Vec<String> = {
        let _ = READINGS_BATCH;
        Vec::new()
    };
    let terms = Terms::new(list.clone(), &readings);
    let fixed: Vec<String> = texts.iter().map(|t| terms.fix(t, &[]).0).collect();
    write_stdout(
        &serde_json::json!({"terms": list, "missing": missing, "prompts": prompts, "fixed": fixed}),
    )
}

/// stdin: WAV 경로의 JSON 배열 → stdout `[{text, raw, words, raw_words, ms}]`. 회의가 쓰는 것과 같은 힌트 프롬프트
/// (`hint_prompt`, 색인의 용어)로 이 Mac에서 음성 인식을 하고 용어 고치기(`terms`)를 거쳐요. `raw`는 고치기 전
/// 글이고 `ms`는 음성 인식 시간이에요. 빈 글은 말이 없다고 거른 거예요.
pub fn run_stt_local(args: crate::SttLocalArgs) -> anyhow::Result<()> {
    use tacie_engine::local_stt::{Device as LDevice, HINT_MAX_TOKENS, LocalWhisper, hint_prompt};
    use tacie_engine::terms::{Readings, Terms};
    let paths: Vec<PathBuf> =
        serde_json::from_str(&read_stdin()?).context("stdin is not a JSON array of paths")?;
    let index = Index::open(&args.index_dir, args.mask, &HashSet::new())
        .with_context(|| format!("open index {}", args.index_dir.display()))?;
    let list = terms_list(args.terms_file.as_deref(), &index)?;
    let terms = Terms::new(list.clone(), &Readings::load(&args.readings));
    let prompt = if let Some(p) = &args.prompt { p.clone() } else { match args.hint {
        crate::SttHintArg::Capped => hint_prompt(&list, HINT_MAX_TOKENS),
        crate::SttHintArg::None => String::new(),
        crate::SttHintArg::Neutral => hint_prompt(&[], HINT_MAX_TOKENS),
        crate::SttHintArg::Full => hint_prompt(&list, usize::MAX),
    } };
    eprintln!(
        "hint: {} terms in {} chars",
        prompt.matches(", ").count() + 1,
        prompt.chars().count()
    );
    let device = match args.device {
        crate::SttDeviceArg::Metal => LDevice::Metal,
        crate::SttDeviceArg::Cpu => LDevice::Cpu,
    };
    let mut whisper = LocalWhisper::load(&args.model, device)?;
    whisper.set_language((args.language != "auto").then(|| args.language.clone()));
    let word = |w: &tacie_engine::services::SpokenWord| serde_json::json!({"text": w.text, "start": w.start, "end": w.end});
    let mut out = Vec::new();
    for p in &paths {
        let wav = std::fs::read(p).with_context(|| format!("read {}", p.display()))?;
        let t = whisper.transcribe(&wav, &prompt)?;
        let (text, words) = terms.fix(&t.text, &t.words);
        out.push(serde_json::json!({
            "text": text, "raw": t.text, "ms": t.ms,
            "words": words.iter().map(word).collect::<Vec<_>>(),
            "raw_words": t.words.iter().map(word).collect::<Vec<_>>(),
        }));
    }
    write_stdout(&out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn folder_snapshot_has_regular_files_without_git_or_links() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        std::fs::create_dir_all(root.join("docs/결정")).unwrap();
        std::fs::create_dir_all(root.join(".git")).unwrap();
        std::fs::create_dir_all(root.join("graft/.graph")).unwrap();
        std::fs::write(root.join("README.md"), "# a").unwrap();
        std::fs::write(root.join("docs/결정/0001.md"), "# b").unwrap();
        std::fs::write(root.join("docs-z.md"), "# z").unwrap();
        std::fs::write(root.join(".git/HEAD"), "ref").unwrap();
        std::fs::write(root.join("graft/.graph/wiring.json"), "{\"nodes\": []}").unwrap();
        std::os::unix::fs::symlink("/etc/hosts", root.join("link.md")).unwrap();
        // 자르지 않는 파일은 읽지 않아요: 읽을 수 없는 파일이어도 실패하지 않아요
        std::fs::write(root.join("image.png"), b"\x89PNG").unwrap();
        std::fs::write(root.join("secret.bin"), b"x").unwrap();
        std::fs::set_permissions(
            root.join("secret.bin"),
            std::os::unix::fs::PermissionsExt::from_mode(0o000),
        )
        .unwrap();
        let snap = folder_snapshot(root).unwrap();
        assert!(!snap.contents.contains_key("image.png"));
        // 경로 바이트 순서예요: `docs-z.md`('-' 0x2d)가 `docs/…`('/' 0x2f)보다 앞이에요
        assert_eq!(
            snap.tree.files(),
            [
                "README.md",
                "docs-z.md",
                "docs/결정/0001.md",
                "graft/.graph/wiring.json"
            ]
        );
        assert_eq!(snap.contents["README.md"], b"# a");
        assert_eq!(snap.tree.entries[0].sha, blob_sha(b"# a"));
        assert_eq!(snap.graph.as_deref(), Some(&b"{\"nodes\": []}"[..]));
        assert_eq!(snap.tree.commit, "");
        let chunks = tacie_sources::repo::chunks(&snap, None, &[], None).unwrap();
        let paths: Vec<&str> = chunks.iter().map(|c| c["path"].as_str().unwrap()).collect();
        assert_eq!(paths, ["README.md", "docs-z.md", "docs/결정/0001.md"]);
    }
}
