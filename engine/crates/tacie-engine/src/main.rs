//! The Tacie search engine. With no subcommand it runs as a server (this is how the app launches it).
//! `candidates` and `embed` are dev commands used by the eval scripts (stdin/stdout JSON, no HTTP).

mod cli;

use std::collections::HashMap;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::sync::{Arc, Mutex, OnceLock, Weak};
use std::time::Instant;

use anyhow::Context;
use clap::{Args, Parser, Subcommand, ValueEnum};
use tacie_embed::{Device, Embed, OnnxEmbedder};
use tacie_engine::jev::Jev;
use tacie_engine::keys::Keys;
use tacie_engine::local_stt::{self, LocalWhisper};
use tacie_engine::openrouter::OpenRouter;
use tacie_engine::pipeline::{Engine, GpuLoad};
use tacie_engine::refresh::{OpenIndex, Places, Refresher, Sources, seed_index};
use tacie_engine::routed_stt::{LoadModel, LocalModel, RoutedStt};
use tacie_engine::sample::{MissingModel, Sample};
use tacie_engine::server::{self, AppState};
use tacie_engine::services::Services;
use tacie_engine::settings;
use tacie_index::{Index, Mask, recover};
use tacie_sources::asana::{self, AsanaSource};
use tacie_sources::atlassian::{self, AtlassianSource};
use tacie_sources::dropbox::{self, DropboxSource};
use tacie_sources::gdrive::{self, GDriveSource};
use tacie_sources::github::GitHub;
use tacie_sources::github::gh_token::SystemGhToken;
use tacie_sources::github::http::API;
use tacie_sources::linear::{self, LinearSource};
use tacie_sources::notion::{self, NotionSource};
use tacie_sources::slack::{self, SlackSource};

#[derive(Parser)]
#[command(
    name = "tacie-engine",
    version,
    about = "Tacie search engine. Without a subcommand it runs the server.",
    args_conflicts_with_subcommands = true
)]
struct Cli {
    #[command(subcommand)]
    cmd: Option<Cmd>,
    #[command(flatten)]
    serve: ServeArgs,
}

#[derive(Subcommand)]
enum Cmd {
    /// Run the HTTP server (the default)
    Serve(ServeArgs),
    /// Dev: read questions as JSON on stdin, print their 50 candidate chunk ids as JSON
    Candidates(CandArgs),
    /// Dev: read a JSON array of strings on stdin, print their 1024-dim vectors as JSON
    Embed(EmbedArgs),
    /// Dev: build an index from a plain folder (no git): the onboarding's sample index
    IndexFolder(IndexFolderArgs),
    /// Eval: print the speaker embedding of each WAV (hidden from --help)
    #[command(hide = true)]
    VoiceEmbed(VoiceEmbedArgs),
    /// Eval: fix team terms in transcripts the way a meeting does (hidden from --help)
    #[command(hide = true)]
    FixTerms(FixTermsArgs),
    /// Eval: transcribe WAV files on this Mac the way a meeting does (hidden from --help)
    #[command(hide = true)]
    SttLocal(SttLocalArgs),
}

#[derive(Args)]
pub struct SttLocalArgs {
    /// ggml whisper model file
    #[arg(long)]
    pub model: PathBuf,
    #[arg(long, value_enum, default_value = "metal")]
    pub device: SttDeviceArg,
    #[arg(long)]
    pub index_dir: PathBuf,
    /// The term readings file, read only (as in fix-terms)
    #[arg(long)]
    pub readings: PathBuf,
    /// superpowers, plans or none
    #[arg(long, default_value = "plans")]
    pub mask: Mask,
    /// capped (the meeting's hint), none (empty prompt; the engine swaps in the neutral one), neutral (the prompt for an index without terms) or full (every term, no cap)
    #[arg(long, value_enum, default_value = "capped")]
    pub hint: SttHintArg,
    /// Use these terms (one per line) instead of the index's, as if a team index held them
    #[arg(long)]
    pub terms_file: Option<PathBuf>,
    /// The language whisper is told: a code such as ko or en, or auto to let whisper detect it
    #[arg(long, default_value = "ko")]
    pub language: String,
    /// Replace the prompt with this text (measuring a prompt in another language)
    #[arg(long)]
    pub prompt: Option<String>,
}

#[derive(Clone, Copy, ValueEnum)]
pub enum SttHintArg {
    Capped,
    None,
    Neutral,
    Full,
}

#[derive(Clone, Copy, ValueEnum)]
pub enum SttDeviceArg {
    Metal,
    Cpu,
}

#[derive(Args)]
pub struct FixTermsArgs {
    #[arg(long)]
    pub index_dir: PathBuf,
    /// The term readings file (the engine keeps it at <data>/term_readings.json). Read only: terms
    /// without readings are listed in `missing` with the prompts to ask for them.
    #[arg(long)]
    pub readings: PathBuf,
    /// superpowers, plans or none
    #[arg(long, default_value = "plans")]
    pub mask: Mask,
    /// Use these terms (one per line) instead of the index's, as if a team index held them
    #[arg(long)]
    pub terms_file: Option<PathBuf>,
}

#[derive(Args)]
pub struct VoiceEmbedArgs {
    /// 16 kHz mono 16-bit WAV files. One JSON array of 512 floats per line, in order (several files
    /// per run keep the eval from reloading the model for every utterance); `null` when too short.
    #[arg(required = true)]
    pub wavs: Vec<PathBuf>,
    /// [default: ~/Library/Application Support/Tacie/models/eres2net-speaker/model.onnx]
    #[arg(long, env = "TACIE_VOICE_MODEL")]
    pub voice_model: Option<PathBuf>,
    /// The speaker-grouping model (fine-tuned CAM++) alone instead of ERes2Net, as the iPhone runs it: 192 floats.
    #[arg(long)]
    pub diar_only: Option<PathBuf>,
}

#[derive(Args)]
pub struct IndexFolderArgs {
    /// The folder to index (every regular file under it, `.git` left out)
    pub folder: PathBuf,
    #[arg(long)]
    pub out: PathBuf,
    /// sample/questions.json: copied as <out>/questions.json ([{text, use}])
    #[arg(long)]
    pub questions: Option<PathBuf>,
    /// [default: ~/Library/Application Support/Tacie/models/bge-m3-onnx]
    #[arg(long, env = "TACIE_MODEL_DIR")]
    pub model_dir: Option<PathBuf>,
}

#[derive(Args)]
struct ServeArgs {
    #[arg(long, default_value = "index")]
    index_dir: PathBuf,
    /// Sources, caches and repos.json
    #[arg(long, default_value = "data")]
    data_dir: PathBuf,
    /// [default: ~/Library/Application Support/Tacie/models/bge-m3-onnx]
    #[arg(long, env = "TACIE_MODEL_DIR")]
    model_dir: Option<PathBuf>,
    /// Speaker embedding model (ERes2Net ONNX); without it utterances get no voice name
    /// [default: ~/Library/Application Support/Tacie/models/eres2net-speaker/model.onnx]
    #[arg(long, env = "TACIE_VOICE_MODEL")]
    voice_model: Option<PathBuf>,
    /// Speaker model for grouping voices in a meeting (fine-tuned CAM++ ONNX); without it, or with `off`, ERes2Net groups them too
    /// [default: ~/Library/Application Support/Tacie/models/campp-ft-speaker/model.onnx]
    #[arg(long, env = "TACIE_DIAR_MODEL")]
    diar_model: Option<PathBuf>,
    #[arg(long, default_value_t = 0)]
    port: u16,
    /// superpowers, plans or none
    #[arg(long, default_value = "plans")]
    mask: Mask,
    #[arg(long)]
    repo_dir: Option<PathBuf>,
    #[arg(long)]
    exit_on_stdin_eof: bool,
    /// The onboarding's sample index (docs/11 12절): used only by requests that say `sample: true`
    #[arg(long)]
    sample_dir: Option<PathBuf>,
}

#[derive(Args)]
pub struct CandArgs {
    #[arg(long)]
    pub index_dir: PathBuf,
    /// [default: ~/Library/Application Support/Tacie/models/bge-m3-onnx]
    #[arg(long, env = "TACIE_MODEL_DIR")]
    pub model_dir: Option<PathBuf>,
    /// superpowers, plans or none
    #[arg(long, default_value = "plans")]
    pub mask: Mask,
    #[arg(long, value_enum, default_value_t = DeviceArg::Cpu)]
    pub device: DeviceArg,
}

#[derive(Args)]
pub struct EmbedArgs {
    /// [default: ~/Library/Application Support/Tacie/models/bge-m3-onnx]
    #[arg(long, env = "TACIE_MODEL_DIR")]
    pub model_dir: Option<PathBuf>,
    /// Embed as index passages (cut at 512 tokens) instead of questions
    #[arg(long)]
    pub passage: bool,
}

#[derive(Clone, Copy, ValueEnum)]
pub enum DeviceArg {
    Coreml,
    Cpu,
}

fn main() -> ExitCode {
    let cli = Cli::parse();
    let (name, done) = match cli.cmd.unwrap_or(Cmd::Serve(cli.serve)) {
        Cmd::Serve(args) => ("serve", run_serve(args)),
        Cmd::Candidates(args) => ("candidates", cli::run_candidates(args)),
        Cmd::Embed(args) => ("embed", cli::run_embed(args)),
        Cmd::IndexFolder(args) => ("index-folder", cli::run_index_folder(args)),
        Cmd::VoiceEmbed(args) => ("voice-embed", cli::run_voice_embed(args)),
        Cmd::FixTerms(args) => ("fix-terms", cli::run_fix_terms(args)),
        Cmd::SttLocal(args) => ("stt-local", cli::run_stt_local(args)),
    };
    match done {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            // 부르는 쪽이 stderr 마지막 줄을 이유로 보여 줘서 한 줄로 써요.
            eprintln!("{name}: {}", format!("{e:#}").replace('\n', " "));
            ExitCode::from(1)
        }
    }
}

/// 토큰 → 모델 → 색인과 임베더 → 포트를 묶고 stdout에 `{"port": N, "load_ms": M}` 한 줄 → 계속 돌아요.
/// stdout에는 이 한 줄만 써요(앱이 첫 줄을 읽어요). 나머지는 모두 stderr예요.
/// 갱신의 문단 임베딩을 Apple GPU(MLX)로 해요. 모델이 없거나, Apple Silicon이 아니거나, `TACIE_GPU=0`이면 CPU로만
/// 해요. 올리는 건 첫 갱신 때예요(`Engine::gpu_embedder`).
fn gpu_load(model: Option<&Path>) -> Option<GpuLoad> {
    let dir = model?.to_path_buf();
    if std::env::var_os("TACIE_GPU").is_some_and(|v| v == "0") {
        eprintln!("embed: gpu off (TACIE_GPU=0)");
        return None;
    }
    #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
    {
        Some(Arc::new(move || {
            Ok(Arc::new(tacie_embed::GpuEmbedder::load(&dir)?) as Arc<dyn Embed>)
        }))
    }
    #[cfg(not(all(target_os = "macos", target_arch = "aarch64")))]
    {
        let _ = dir;
        None
    }
}

fn run_serve(args: ServeArgs) -> anyhow::Result<()> {
    let Some(token) = std::env::var("TACIE_ENGINE_TOKEN")
        .ok()
        .filter(|t| !t.is_empty())
    else {
        eprintln!("TACIE_ENGINE_TOKEN is not set");
        std::process::exit(1);
    };
    // 모델이 아직 없으면(첫 실행에 받는 중) 그래도 떠요: 예시 질문은 답하고, 임베딩이 필요한 요청은 `model`로 실패해요.
    // 다 받으면 앱이 엔진을 다시 띄워요.
    let model = cli::find_model(cli::model_dir(args.model_dir));

    let t = Instant::now();
    let config_dir = settings::config_dir();
    // AI 키는 앱이 `POST /ai/credentials`로 넘겨요(메모리에만 둬요). 키 파일(~/.config/{typesafe,openrouter})은 개발
    // 앱이 띄운 엔진(TACIE_KEY_FILES=1)만 읽어요: 엔진은 늘 release로 빌드되니 띄우는 쪽이 정해요(GitHub의 gh와 같아요).
    let key_files = std::env::var_os("TACIE_KEY_FILES").is_some_and(|v| v == "1");
    let keys = Arc::new(Keys::new(key_files.then(|| config_dir.clone())));
    // 깨진 키 파일은 조용히 "없음"이 되니 이유를 stderr에 남겨요(내용은 쓰지 않아요).
    for line in settings::warn_unreadable(&config_dir, key_files) {
        eprintln!("{line}");
    }
    // 바꿔 넣기가 두 이름 바꾸기 사이에서(죽거나 전원이 나가서) 끊겼으면 index/가 없어요: 읽기 전에 되돌려요.
    recover(&args.index_dir).with_context(|| format!("put back {}", args.index_dir.display()))?;
    // 그 자리에 아무것도 없을 때만 빈 씨앗으로 시작해요. 저장소는 앱이 /index/rebuild로 골라요. 깨진 색인은 덮지
    // 않아서 아래 Index::open이 실패하고 종료 코드 1이에요.
    if seed_index(&args.index_dir)? {
        eprintln!(
            "index: none at {}, started empty: choose a repository to sync",
            args.index_dir.display()
        );
    }
    // 비공개 슬랙 채널은 그 멤버에게만 보여요: 보는 사람은 ~/.config/slack/config.json의 슬랙 사용자예요.
    let viewer = settings::viewer(&config_dir);
    let index = Index::open(&args.index_dir, args.mask, &viewer)
        .with_context(|| format!("open index {}", args.index_dir.display()))?;
    let embedder: Arc<dyn Embed> = match &model {
        Some(dir) => Arc::new(OnnxEmbedder::load(dir, Device::Cpu)?),
        None => Arc::new(MissingModel),
    };
    // 예시 자료는 없거나 깨져도 엔진은 떠요: 연습만 못 해요.
    let sample = args
        .sample_dir
        .as_deref()
        .and_then(|dir| match Sample::open(dir) {
            Ok(s) => Some(s),
            Err(e) => {
                eprintln!("sample: {}", format!("{e:#}").replace('\n', " "));
                None
            }
        });
    // ort/CoreML이 줄바꿈 없이 남긴 글이 있어도 엔진이 쓰는 줄은 새 줄에서 시작해요.
    eprintln!();

    // 환경 변수 중 UTF-8이 아닌 건 덮어쓰기가 될 수 없어서 건너뛰어요(env::vars()는 그런 값에서 멈춰요).
    let env: HashMap<String, String> = std::env::vars_os()
        .filter_map(|(k, v)| Some((k.into_string().ok()?, v.into_string().ok()?)))
        .collect();
    let settings = settings::resolve(&env);
    let last = Arc::new(Mutex::new(HashMap::new()));
    // Jev와 답 모델이 같은 키를 봐요(`/ai/credentials`가 바꾸면 둘 다 다음 요청부터 써요).
    let jev = Arc::new(Jev::new(keys.clone(), &settings, &env, last.clone()));
    let openrouter = Arc::new(OpenRouter::with_keys(keys, &settings, &env, last.clone()));
    // 받아쓰기는 발화마다 서버(OpenRouter)와 이 Mac의 모델 중 하나로 가요(`POST /stt`가 정해요). 프롬프트의 팀 용어와
    // 동기화 상태는 아래에서 만드는 엔진·갱신기가 알아서, 여기서는 아직 없어요: 나중에 채우는 `OnceLock<Weak<..>>`로
    // 읽어요(강한 참조면 엔진이 서비스를, 서비스가 엔진을 붙들어 영영 안 풀려요).
    let engine_cell: Arc<OnceLock<Weak<Engine>>> = Arc::new(OnceLock::new());
    let refresher_cell: Arc<OnceLock<Weak<Refresher>>> = Arc::new(OnceLock::new());
    let load: LoadModel = Arc::new(|path, device| {
        let mut whisper = LocalWhisper::load(path, device)?;
        whisper.set_language(tacie_engine::lang::whisper_language(tacie_engine::lang::mac_lang()));
        Ok(Arc::new(whisper) as Arc<dyn LocalModel>)
    });
    let routed = {
        let (engine_cell, refresher_cell) = (engine_cell.clone(), refresher_cell.clone());
        RoutedStt::new(
            openrouter.clone(),
            load,
            Arc::new(move || {
                refresher_cell
                    .get()
                    .and_then(Weak::upgrade)
                    .is_some_and(|r| r.status()["state"] == "running")
            }),
            Arc::new(move || {
                engine_cell
                    .get()
                    .and_then(Weak::upgrade)
                    .map(|e| {
                        local_stt::hint_prompt(&e.term_fixer().list, local_stt::HINT_MAX_TOKENS)
                    })
                    .unwrap_or_default()
            }),
        )
    };
    // 판단과 리랭크는 Jev, 답·장부는 OpenRouter, 받아쓰기는 위의 경로예요.
    let services = Services {
        reranker: jev.clone(),
        gater: jev.clone(),
        answerer: openrouter.clone(),
        transcriber: routed.clone(),
        note_writer: openrouter.clone(),
        panel: openrouter,
        meeting_search: None,
    };
    // 카드는 색인이 이 체크아웃의 저장소로 만든 것일 때만 로컬 파일을 가리켜요(origin은 여기서 한 번 읽어요).
    let engine = Arc::new(
        Engine::new(index, embedder, services, settings, jev, last)
            .with_repo_dir(args.repo_dir)
            .with_config_dir(config_dir.clone())
            .with_sample(sample)
            .with_model_ready(model.is_some())
            .with_voice(cli::load_voice(args.voice_model, args.diar_model))
            .with_gpu(gpu_load(model.as_deref()))
            .with_terms_file(Some(args.data_dir.join("term_readings.json")))
            .with_stt(Some(routed.clone())),
    );
    let _ = engine_cell.set(Arc::downgrade(&engine));
    // GitHub 연결은 TACIE_GITHUB_MODE로 시작해요. 없으면 닫힌 쪽(none)이에요: 엔진은 늘 release로 빌드되니 빌드
    // 종류가 아니라 띄우는 쪽이 정해요(개발 앱만 gh를 줘요). gh, none 밖의 값(토큰 없는 token, 빈 값, UTF-8이 아닌
    // 값)도 none이에요. gh로 시작하지 않은 엔진은 나중에도 gh 모드가 되지 않아요(`gh_allowed`).
    let mode = match std::env::var_os("TACIE_GITHUB_MODE") {
        None => "none".to_string(),
        Some(v) => v.into_string().unwrap_or_else(|_| "none".to_string()),
    };
    // REST 주소예요: 테스트·계약은 TACIE_GITHUB_API_URL로 loopback 가짜를 가리켜요(release는 loopback만). 런타임
    // 전에 만들어요(blocking 클라이언트).
    let api = settings::service_url(&env, "TACIE_GITHUB_API_URL", cfg!(debug_assertions))
        .unwrap_or(API.into());
    let gh_token = Arc::new(SystemGhToken::new());
    let github = match GitHub::new(&mode, &api, gh_token.clone()) {
        Ok(github) => github,
        Err(_) => {
            eprintln!("config: TACIE_GITHUB_API_URL ignored: it is not a web address");
            GitHub::new(&mode, API, gh_token)?
        }
    };
    let github = Arc::new(github);
    // 갱신이 여는 새 색인은 시작할 때의 mask와 보는 사람으로 읽어요.
    let mask = args.mask;
    let open_index: OpenIndex = Arc::new(move |d: &std::path::Path| Index::open(d, mask, &viewer));
    let refresher = Arc::new(Refresher::new(
        engine.clone(),
        args.index_dir.clone(),
        Places::new(&args.data_dir),
        Sources {
            github: github.clone(),
            slack: Arc::new(SlackSource {
                config_dir,
                base: slack::API.to_string(),
            }),
            notion: Arc::new(NotionSource::new(notion::API)),
            linear: Arc::new(LinearSource::new(linear::API)),
            gdrive: Arc::new(GDriveSource::new(gdrive::API)),
            atlassian: Arc::new(AtlassianSource::new(atlassian::api::API)),
            asana: Arc::new(AsanaSource::new(asana::api::API)),
            dropbox: Arc::new(DropboxSource::new(dropbox::API, dropbox::CONTENT)),
        },
        open_index,
    ));
    let _ = refresher_cell.set(Arc::downgrade(&refresher));
    let state = Arc::new(AppState {
        engine,
        token,
        github,
        refresher: Some(refresher),
        meeting_dir: None,
    });

    let runtime = tokio::runtime::Runtime::new().context("start the async runtime")?;
    runtime.block_on(async {
        // 동기화가 끝나면 받아쓰기의 CPU 컨텍스트를 다음 발화까지 붙들지 않고 내려요.
        tokio::spawn(async move {
            let mut every = tokio::time::interval(std::time::Duration::from_secs(15));
            loop {
                every.tick().await;
                routed.release_idle_cpu();
            }
        });
        let (addr, serving) = server::serve(state, args.port)
            .await
            .with_context(|| format!("bind 127.0.0.1:{}", args.port))?;
        let load_ms = (t.elapsed().as_secs_f64() * 1000.0).round() as u64;
        let mut stdout = std::io::stdout().lock();
        writeln!(
            stdout,
            "{{\"port\": {}, \"load_ms\": {load_ms}}}",
            addr.port()
        )?;
        stdout.flush()?;
        drop(stdout);
        if args.exit_on_stdin_eof {
            // 앱이 닫히면 stdin이 닫혀요. 엔진이 앱보다 오래 남지 않게 그때 끝나요.
            std::thread::spawn(|| {
                let _ = std::io::copy(&mut std::io::stdin().lock(), &mut std::io::sink());
                std::process::exit(0);
            });
        }
        serving.await.context("serve")
    })
}
