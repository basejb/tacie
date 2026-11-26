//! Measures the cost of loading one more CPU context while a Metal context is loaded (spec 5-2 "to check").
//! Usage: `TACIE_STT_MODEL_FILE=<ggml model> cargo run --release --example stt_ctx_bench -p tacie-engine`

use std::path::PathBuf;
use std::time::Instant;

use tacie_engine::local_stt::{Device, LocalWhisper};

/// `ri_phys_footprint`(활동 모니터의 "메모리")를 바이트로 읽어요.
fn footprint() -> u64 {
    #[repr(C)]
    #[derive(Default)]
    struct RusageInfoV2 {
        uuid: [u8; 16],
        fields: [u64; 7],
        phys_footprint: u64,
        rest: [u64; 10],
    }
    unsafe extern "C" {
        fn proc_pid_rusage(pid: i32, flavor: i32, buf: *mut RusageInfoV2) -> i32;
        fn getpid() -> i32;
    }
    let mut info = RusageInfoV2::default();
    // flavor 2 = RUSAGE_INFO_V2
    let rc = unsafe { proc_pid_rusage(getpid(), 2, &mut info) };
    if rc == 0 { info.phys_footprint } else { 0 }
}

fn mb(b: u64) -> f64 {
    b as f64 / 1_048_576.0
}

fn main() {
    let Some(model) = std::env::var_os("TACIE_STT_MODEL_FILE").map(PathBuf::from) else {
        eprintln!("TACIE_STT_MODEL_FILE이 필요해요");
        return;
    };
    let f0 = footprint();
    let t = Instant::now();
    let metal = LocalWhisper::load(&model, Device::Metal).expect("Metal 로드");
    let metal_s = t.elapsed().as_secs_f64();
    let f1 = footprint();
    let t = Instant::now();
    let cpu = LocalWhisper::load(&model, Device::Cpu).expect("CPU 로드");
    let cpu_s = t.elapsed().as_secs_f64();
    let f2 = footprint();
    println!(
        "Metal load: {metal_s:.2} s, footprint {:.0} MB -> {:.0} MB",
        mb(f0),
        mb(f1)
    );
    println!(
        "CPU load (Metal 올라간 상태): {cpu_s:.2} s, footprint {:.0} MB -> {:.0} MB (+{:.0} MB)",
        mb(f1),
        mb(f2),
        mb(f2.saturating_sub(f1))
    );
    drop((metal, cpu));
}
