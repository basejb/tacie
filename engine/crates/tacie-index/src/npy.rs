//! Reads and writes only 2-D f32 numpy `.npy` files (`<f4`, C order, version 1.0). emb.npy has this shape.

use std::path::Path;

use anyhow::{Context, bail, ensure};

const MAGIC: &[u8] = b"\x93NUMPY";
/// 매직 6바이트 + 버전 2바이트 + 헤더 길이 2바이트예요.
const PREFIX: usize = 10;
/// numpy가 헤더를 맞추는 단위예요.
const ALIGN: usize = 64;
/// numpy가 행 수가 늘어날 자리로 남기는 칸이에요(`GROWTH_AXIS_MAX_DIGITS`). 같은 바이트를 쓰려고 따라 해요.
const GROWTH_DIGITS: usize = 21;

/// (행 우선 데이터, 행 수, 열 수)예요.
pub fn read_f32_2d(path: &Path) -> anyhow::Result<(Vec<f32>, usize, usize)> {
    let bytes = std::fs::read(path).with_context(|| format!("read {}", path.display()))?;
    parse(&bytes).with_context(|| format!("{} is not a 2-D float32 npy", path.display()))
}

pub fn write_f32_2d(path: &Path, data: &[f32], rows: usize, cols: usize) -> anyhow::Result<()> {
    ensure!(
        rows.checked_mul(cols) == Some(data.len()),
        "npy shape ({rows}, {cols}) does not match {} values",
        data.len()
    );
    let mut header =
        format!("{{'descr': '<f4', 'fortran_order': False, 'shape': ({rows}, {cols}), }}");
    header.push_str(&" ".repeat(GROWTH_DIGITS.saturating_sub(rows.to_string().len())));
    // 앞머리 10바이트 + 헤더(공백으로 채우고 마지막은 \n)가 64의 배수가 되게 맞춰요.
    // 이미 맞아도 64칸을 더 채우는 것까지 numpy와 같아요.
    let pad = ALIGN - (PREFIX + header.len() + 1) % ALIGN;
    header.push_str(&" ".repeat(pad));
    header.push('\n');
    let header_len = u16::try_from(header.len()).context("npy header too long for version 1.0")?;

    let mut out = Vec::with_capacity(PREFIX + header.len() + data.len() * 4);
    out.extend_from_slice(MAGIC);
    out.extend_from_slice(&[1, 0]);
    out.extend_from_slice(&header_len.to_le_bytes());
    out.extend_from_slice(header.as_bytes());
    for v in data {
        out.extend_from_slice(&v.to_le_bytes());
    }
    std::fs::write(path, out).with_context(|| format!("write {}", path.display()))
}

fn parse(bytes: &[u8]) -> anyhow::Result<(Vec<f32>, usize, usize)> {
    ensure!(
        bytes.len() >= PREFIX && bytes.starts_with(MAGIC),
        "missing the npy magic"
    );
    ensure!(
        bytes[6..8] == [1, 0],
        "npy version {}.{} (only 1.0 is read)",
        bytes[6],
        bytes[7]
    );
    let header_len = u16::from_le_bytes([bytes[8], bytes[9]]) as usize;
    let start = PREFIX + header_len;
    ensure!(
        bytes.len() >= start,
        "npy header runs past the end of the file"
    );
    let header = std::str::from_utf8(&bytes[PREFIX..start]).context("npy header is not text")?;

    let descr = value_of(header, "descr")?;
    ensure!(descr == "'<f4'", "dtype {descr} (only '<f4' is read)");
    let fortran = value_of(header, "fortran_order")?;
    ensure!(
        fortran == "False",
        "fortran_order {fortran} (only False is read)"
    );
    let (rows, cols) = shape(value_of(header, "shape")?)?;

    let body = &bytes[start..];
    let want = rows.checked_mul(cols).and_then(|n| n.checked_mul(4));
    ensure!(
        want == Some(body.len()),
        "npy shape ({rows}, {cols}) needs {} data bytes, file has {}",
        rows.saturating_mul(cols).saturating_mul(4),
        body.len()
    );
    let (words, _) = body.as_chunks::<4>(); // 길이를 위에서 맞춰서 남는 바이트는 없어요
    let data = words.iter().map(|&b| f32::from_le_bytes(b)).collect();
    Ok((data, rows, cols))
}

/// 헤더 dict에서 `'key': ` 뒤의 값 글자예요. 튜플은 괄호째, 나머지는 다음 쉼표까지예요.
fn value_of<'a>(header: &'a str, key: &str) -> anyhow::Result<&'a str> {
    let tag = format!("'{key}':");
    let Some(at) = header.find(&tag) else {
        bail!("npy header has no '{key}'")
    };
    let rest = header[at + tag.len()..].trim_start();
    let end = if rest.starts_with('(') {
        rest.find(')').map(|i| i + 1)
    } else {
        rest.find([',', '}'])
    };
    let Some(end) = end else {
        bail!("npy header value for '{key}' is not closed")
    };
    Ok(rest[..end].trim())
}

fn shape(tuple: &str) -> anyhow::Result<(usize, usize)> {
    let inner = tuple
        .strip_prefix('(')
        .and_then(|t| t.strip_suffix(')'))
        .with_context(|| format!("shape {tuple} is not a tuple"))?;
    let dims: Vec<usize> = inner
        .split(',')
        .map(str::trim)
        .filter(|d| !d.is_empty())
        .map(|d| {
            d.parse()
                .with_context(|| format!("shape {tuple} has a bad size {d:?}"))
        })
        .collect::<anyhow::Result<_>>()?;
    match dims[..] {
        [rows, cols] => Ok((rows, cols)),
        _ => bail!("shape {tuple} is not 2-D"),
    }
}
