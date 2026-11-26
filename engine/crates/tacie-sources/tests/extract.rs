//! Checks file text extraction (extract.rs, issue #31) against handmade samples. The samples are documents made for
//! testing.

use tacie_sources::extract::{Fail, FileKind, text};

fn fixture(name: &str) -> Vec<u8> {
    std::fs::read(format!("{}/tests/fixtures/extract/{name}", env!("CARGO_MANIFEST_DIR"))).unwrap()
}

#[test]
fn pdf_keeps_korean_words_apart() {
    let t = text(fixture("policy.pdf"), FileKind::Pdf).unwrap();
    assert!(t.contains("결제 웹훅 재시도 정책"), "{t}");
    assert!(t.contains("담당자는 지윤 님이에요"));
    assert!(t.contains("세션 만료 시간은 12시간"));
    // macOS 인쇄로 만든 PDF의 한글도 읽어요
    let t = text(fixture("note.pdf"), FileKind::Pdf).unwrap();
    assert!(t.contains("로그인 세션 만료는 12시간이에요"), "{t}");
}

#[test]
fn a_scanned_pdf_has_no_text() {
    assert_eq!(text(fixture("scan.pdf"), FileKind::Pdf), Err(Fail::NoText));
}

#[test]
fn office_files_come_out_as_markdown() {
    let t = text(fixture("doc.docx"), FileKind::Docx).unwrap();
    assert!(t.contains("# 결제 웹훅 재시도 정책"), "{t}");
    assert!(t.contains("| 재시도 횟수 | 5 |"));
    let t = text(fixture("deck.pptx"), FileKind::Pptx).unwrap();
    assert!(t.contains("## 세션 만료 정책"), "{t}");
    assert!(t.contains("12시간으로 정했어요"));
    assert!(t.contains("Payfold 결정 배경"), "발표자 노트도 들어가요");
    let t = text(fixture("sheet.xlsx"), FileKind::Xlsx).unwrap();
    assert!(t.contains("## 결정") && t.contains("## 예산"), "{t}");
    assert!(t.contains("| 세션 | 12시간 |"));
}

#[test]
fn a_damaged_file_fails_alone() {
    let mut b = fixture("doc.docx");
    b.truncate(b.len() / 2);
    assert_eq!(text(b, FileKind::Docx), Err(Fail::Broken));
    // 종류가 틀려도 그 파일 하나의 실패예요
    assert!(text(fixture("doc.docx"), FileKind::Pdf).is_err());
}
