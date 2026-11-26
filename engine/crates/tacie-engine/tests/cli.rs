//! The CLI needs a real model and index to run, so here we only check argument parsing and the exit when the model
//! is missing.

use std::process::Command;

fn bin() -> Command {
    Command::new(env!("CARGO_BIN_EXE_tacie-engine"))
}

#[test]
fn help_lists_the_three_subcommands() {
    let out = bin().arg("--help").output().unwrap();
    assert!(out.status.success(), "--help exited {:?}", out.status);
    let text = String::from_utf8_lossy(&out.stdout);
    // clap의 서브커맨드 목록 줄이에요("server"에도 걸리는 부분 문자열 검사는 쓰지 않아요).
    for name in ["serve", "candidates", "embed"] {
        let prefix = format!("  {name}");
        assert!(
            text.lines().any(|l| l.starts_with(&prefix)),
            "--help does not list {name}:\n{text}"
        );
    }
}

#[test]
fn candidates_without_a_model_exits_3() {
    let out = bin()
        .args([
            "candidates",
            "--model-dir",
            "/nonexistent",
            "--index-dir",
            "/x",
        ])
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(3), "status {:?}", out.status);
    let err = String::from_utf8_lossy(&out.stderr);
    assert!(err.contains("model: missing /nonexistent"), "stderr: {err}");
}

#[test]
fn serve_checks_the_token_before_the_model() {
    let out = bin()
        .args(["--model-dir", "/nonexistent", "--index-dir", "/x"])
        .env_remove("TACIE_ENGINE_TOKEN")
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(1), "status {:?}", out.status);
    assert_eq!(
        String::from_utf8_lossy(&out.stderr).trim_end(),
        "TACIE_ENGINE_TOKEN is not set"
    );
    assert!(out.stdout.is_empty());
}

#[test]
fn serve_starts_without_the_model() {
    // 첫 실행에 모델을 받는 동안에도 엔진은 떠요(예시 질문은 답해요, docs/11 12절). 이유는 stderr에 남아요.
    let dir = tempfile::tempdir().unwrap();
    let out = bin()
        .args([
            "serve",
            "--model-dir",
            "/nonexistent",
            "--exit-on-stdin-eof",
            "--index-dir",
        ])
        .arg(dir.path().join("index"))
        .arg("--data-dir")
        .arg(dir.path().join("data"))
        .env("TACIE_ENGINE_TOKEN", "t")
        .stdin(std::process::Stdio::null())
        .output()
        .unwrap();
    assert!(String::from_utf8_lossy(&out.stderr).contains("model: missing /nonexistent"));
    assert!(
        String::from_utf8_lossy(&out.stdout).starts_with("{\"port\": "),
        "stdout {:?}",
        out.stdout
    );
}

#[test]
fn serve_starts_without_the_voice_model() {
    // 화자 임베딩 모델(docs/04 4절)은 없어도 떠요. 이유는 stderr 한 줄이에요.
    let dir = tempfile::tempdir().unwrap();
    let out = bin()
        .args([
            "serve",
            "--model-dir",
            "/nonexistent",
            "--voice-model",
            "/nonexistent/voice.onnx",
            "--exit-on-stdin-eof",
            "--index-dir",
        ])
        .arg(dir.path().join("index"))
        .arg("--data-dir")
        .arg(dir.path().join("data"))
        .env("TACIE_ENGINE_TOKEN", "t")
        .stdin(std::process::Stdio::null())
        .output()
        .unwrap();
    let err = String::from_utf8_lossy(&out.stderr);
    assert!(
        err.contains("voice: off (missing /nonexistent/voice.onnx)"),
        "stderr: {err}"
    );
    assert!(
        String::from_utf8_lossy(&out.stdout).starts_with("{\"port\": "),
        "stdout {:?}",
        out.stdout
    );
}

#[test]
fn a_model_dir_without_onnx_data_counts_as_missing() {
    // 그래프(model.onnx)만 있고 가중치(model.onnx_data)가 없으면 받다 만 모델이에요.
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("model.onnx"), b"").unwrap();
    std::fs::write(dir.path().join("tokenizer.json"), b"").unwrap();
    let out = bin()
        .args(["candidates", "--index-dir", "/x", "--model-dir"])
        .arg(dir.path())
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(3), "status {:?}", out.status);
    let err = String::from_utf8_lossy(&out.stderr);
    assert!(err.contains("model: missing"), "stderr: {err}");
}

/// 색인 폴더가 없으면 빈 씨앗으로 떠요(계획 4 결정 6). 모델이 있어야 돌아서 `TACIE_MODEL_DIR`가 없으면 건너뛰어요.
#[test]
fn serve_without_an_index_starts() {
    use std::io::{BufRead, BufReader};
    use std::process::Stdio;
    use std::sync::mpsc;
    use std::time::Duration;

    let Some(model) = std::env::var_os("TACIE_MODEL_DIR") else {
        eprintln!("skip: TACIE_MODEL_DIR unset");
        return;
    };
    let dir = tempfile::tempdir_in(env!("CARGO_TARGET_TMPDIR")).unwrap();
    // 진짜 ~/.config(키 파일, 슬랙 사용자)를 읽지 않게 HOME을 임시 폴더로 둬요.
    let mut child = bin()
        .args(["--index-dir", "index", "--data-dir", "data", "--port", "0"])
        .arg("--exit-on-stdin-eof")
        .current_dir(dir.path())
        .env_clear()
        .env("PATH", std::env::var_os("PATH").unwrap_or_default())
        .env("HOME", dir.path())
        .env("TACIE_ENGINE_TOKEN", "t")
        .env("TACIE_MODEL_DIR", &model)
        .env("TACIE_GITHUB_MODE", "none")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let stdout = child.stdout.take().unwrap();
    let (tx, rx) = mpsc::channel();
    std::thread::spawn(move || {
        let mut line = String::new();
        let _ = BufReader::new(stdout).read_line(&mut line);
        let _ = tx.send(line);
    });
    let line = match rx.recv_timeout(Duration::from_secs(300)) {
        Ok(line) => line,
        Err(_) => {
            let _ = child.kill();
            panic!("no port line in 300 s");
        }
    };
    if !line.starts_with("{\"port\": ") {
        let _ = child.kill();
        let out = child.wait_with_output().unwrap();
        panic!(
            "first stdout line {line:?}, stderr: {}",
            String::from_utf8_lossy(&out.stderr)
        );
    }
    assert!(dir.path().join("index/meta.json").is_file());
    assert!(!dir.path().join("index.new").exists());

    drop(child.stdin.take()); // stdin을 닫으면 끝나요
    let deadline = std::time::Instant::now() + Duration::from_secs(30);
    let status = loop {
        if let Some(status) = child.try_wait().unwrap() {
            break status;
        }
        if std::time::Instant::now() > deadline {
            let _ = child.kill();
            panic!("still running 30 s after stdin closed");
        }
        std::thread::sleep(Duration::from_millis(50));
    };
    assert!(status.success(), "exit {status:?}");
    let mut err = String::new();
    std::io::Read::read_to_string(&mut child.stderr.take().unwrap(), &mut err).unwrap();
    assert!(
        err.contains("index: none at index, started empty: choose a repository to sync"),
        "stderr: {err}"
    );
}

#[test]
fn packed_weights_are_unpacked_before_the_model_check() {
    // 받은 fp16 가중치(model.onnx_data.f16)만 있으면 엔진이 먼저 fp32로 풀고 다음 단계로 가요.
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("model.onnx"), b"").unwrap();
    std::fs::write(dir.path().join("tokenizer.json"), b"").unwrap();
    // fp16 1.0 = 0x3C00, -2.0 = 0xC000 (리틀 엔디언)
    std::fs::write(
        dir.path().join("model.onnx_data.f16"),
        [0x00, 0x3C, 0x00, 0xC0],
    )
    .unwrap();
    let out = bin()
        .args(["candidates", "--index-dir", "/nonexistent", "--model-dir"])
        .arg(dir.path())
        .output()
        .unwrap();
    let err = String::from_utf8_lossy(&out.stderr);
    // 모델은 갖춰졌고, 그다음(색인 열기)에서 실패해요.
    assert_ne!(out.status.code(), Some(3), "stderr: {err}");
    assert!(err.contains("model: unpacked in"), "stderr: {err}");
    let weights = std::fs::read(dir.path().join("model.onnx_data")).unwrap();
    let expected: Vec<u8> = [1.0f32, -2.0]
        .iter()
        .flat_map(|x| x.to_le_bytes())
        .collect();
    assert_eq!(weights, expected);
    assert!(!dir.path().join("model.onnx_data.f16").exists());
}

#[test]
fn a_broken_packed_file_counts_as_missing() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("model.onnx"), b"").unwrap();
    std::fs::write(dir.path().join("tokenizer.json"), b"").unwrap();
    std::fs::write(dir.path().join("model.onnx_data.f16"), [0x00, 0x3C, 0x00]).unwrap();
    let out = bin()
        .args(["candidates", "--index-dir", "/x", "--model-dir"])
        .arg(dir.path())
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(3), "status {:?}", out.status);
    let err = String::from_utf8_lossy(&out.stderr);
    assert!(err.contains("model: unpack failed"), "stderr: {err}");
    assert!(!dir.path().join("model.onnx_data").exists());
}

#[test]
fn voice_embed_is_hidden_from_help_but_runs() {
    let out = bin().arg("--help").output().unwrap();
    let text = String::from_utf8_lossy(&out.stdout);
    assert!(
        !text.contains("voice-embed"),
        "--help shows voice-embed:\n{text}"
    );
    // 모델이 없으면 말없이 넘어가지 않고 실패해요(평가가 빈 결과를 점수로 읽지 않게).
    let out = bin()
        .args(["voice-embed", "--voice-model", "/nonexistent", "a.wav"])
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(1), "status {:?}", out.status);
    let err = String::from_utf8_lossy(&out.stderr);
    assert!(err.contains("voice-embed:"), "stderr: {err}");
}
