//! Fixing team terms in transcription (docs/01 "STT에 팀 용어 쓰기 스파이크", docs/04 5절).
//!
//! Transcription often writes "OpenRouter" as "오픈라우터" from the sound alone. Terms the team uses often are pulled
//! from the index (`extract`), each term gets the Hangul spellings it is likely to be written as (`Readings`, made
//! once by the model and kept on this Mac), and those spellings in transcribed text are replaced with the original
//! term (`Terms::fix`). Similar Latin spellings ("Passage tokens") are replaced too.
//!
//! Guards against wrong replacements:
//! - A replaced word can be followed only by a particle (에서, 는, 로 …) or punctuation. "타시겠어요" does not become
//!   "Tacie겠어요".
//! - Spellings of two syllables or fewer are replaced only on an exact match ("다시" is not "타시").
//! - Terms are pulled only from visible chunks (permissions first). GitHub code, docs and issues and Notion, Linear
//!   and Drive documents are looked at; Slack conversations are not included.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::LazyLock;

use regex::Regex;
use serde_json::{Map, Value};
use tacie_index::chunk::Chunk;

use crate::prompts::Prompt;
use crate::services::SpokenWord;

/// 문서에서 뽑는 제품·도구 이름 수, 코드에 선언된 이름 수예요. 문서 이름은 스파이크의 60개에서 노션·Linear·
/// Drive를 더하며 100개로 늘렸어요(60개로는 GitHub 문서의 이름이 밀려났어요).
const DOC_TERMS: usize = 100;
const CODE_TERMS: usize = 40;
/// 이만큼 넘는 조각에 나와야 용어로 쳐요.
const DOC_MIN: usize = 4;
const CODE_MIN: usize = 8;

/// 용어로 치지 않는 흔한 영어 단어예요(문서에 대문자로 자주 나오지만 팀 용어가 아니에요).
const GENERIC: &[&str] = &[
    "Application",
    "Library",
    "Support",
    "Repositories",
    "Contents",
    "Build",
    "Debug",
    "Pro",
    "Space",
    "Core",
    "Cloud",
    "Command",
    "Esc",
    "App",
    "Developer",
    "Resources",
    "Brand",
    "Face",
    "Python",
    "Rust",
    "Swift",
    "Mac",
    "Apple",
    "Google",
    "API",
    "POST",
    "GET",
    "HTTP",
    "JSON",
    "URL",
    "CPU",
    "GPU",
    "CLI",
    "DNS",
    "README",
    "CLAUDE",
    "Worker",
    "Runtime",
    "MacOS",
    "Theme",
    "Seoul",
    "Hugging",
    "Finder",
    "Arc",
    "The",
    "This",
    "For",
    "And",
    "With",
    "From",
    "When",
    "What",
    "One",
    "Default",
    "Error",
    "Errors",
    "Note",
    "Notes",
    "Test",
    "Tests",
    "Audio",
    "Feature",
    "Phase",
    "Desktop",
    "Improvement",
    "Web",
    "Docs",
    "Slides",
    "Server",
    "Growth",
    "Info",
    "Bug",
    "Free",
    "Store",
    "Fonts",
];

/// 한국어 회의에서 한글로 말하고 한글로 적는 흔한 외래어예요("데이터", "도메인"). 문서에 대문자로 나와도 팀
/// 용어로 치지 않아요. 용어가 되면 "데이터를"이 "Data를"로 바뀌었어요(2026-11-11). 대소문자는 가리지 않아요.
/// 소문자로 더 자주 나오는지로 거르면 Tacie·Linear·Slack 같은 이름까지 빠져서(코드에 소문자로 많아요) 목록으로 둬요.
const LOANWORDS: &[&str] = &[
    "data",
    "domain",
    "user",
    "users",
    "model",
    "models",
    "design",
    "issue",
    "issues",
    "version",
    "file",
    "files",
    "project",
    "session",
    "token",
    "tokens",
    "prompt",
    "log",
    "logs",
    "cache",
    "query",
    "field",
    "type",
    "types",
    "list",
    "card",
    "cards",
    "feedback",
    "streaming",
    "plan",
    "plans",
    "keyword",
    "repo",
    "branch",
    "commit",
    "batch",
    "region",
    "component",
    "workspace",
    "task",
    "tasks",
    "changes",
    "console",
    "picker",
    "int",
    "service",
    "account",
    "event",
    "events",
    "option",
    "options",
    "setting",
    "settings",
    "message",
    "page",
    "pages",
    "button",
    "screen",
    "table",
    "status",
    "update",
    "release",
    "search",
    "index",
    "meeting",
    "summary",
    "speaker",
    "content",
    "source",
    "sources",
    "client",
    "admin",
    "team",
    "price",
    "pricing",
    "landing",
    "member",
    "template",
];

/// 한글로 받아쓴 흔한 외래어예요. 용어 표기와 비슷해도(한두 자모 차이) 바꾸지 않아요. 표기와 똑같을 때만 바꿔요.
/// "클래스"가 Glass의 표기 "글래스"와 자모 하나 차이라 "Glass"가 됐어요(2026-11-11).
const HANGUL_LOANWORDS: &[&str] = &[
    "클래스",
    "데이터",
    "도메인",
    "유저",
    "서버",
    "모델",
    "디자인",
    "이슈",
    "버전",
    "파일",
    "프로젝트",
    "세션",
    "토큰",
    "프롬프트",
    "로그",
    "에러",
    "캐시",
    "쿼리",
    "필드",
    "타입",
    "리스트",
    "카드",
    "피드백",
    "스트리밍",
    "플랜",
    "키워드",
    "브랜치",
    "커밋",
    "배치",
    "리전",
    "컴포넌트",
    "테스트",
    "서비스",
    "어카운트",
    "이벤트",
    "옵션",
    "세팅",
    "메시지",
    "페이지",
    "버튼",
    "스크린",
    "테이블",
    "업데이트",
    "릴리스",
    "릴리즈",
    "인덱스",
    "미팅",
    "콘텐츠",
    "소스",
    "클라이언트",
    "어드민",
    "멤버",
    "템플릿",
    "플랫폼",
    "커머스",
    "브랜드",
    "그래프",
    "크래시",
    "글로벌",
    "레벨",
    "모바일",
    "마케팅",
    "타깃",
    "타겟",
    "채널",
    "캔버스",
    "블록",
];

/// 바꾼 용어 뒤에 붙어도 되는 말이에요(조사와 짧은 어미). 이것 말고 다른 글이 붙으면 바꾸지 않아요.
const TAILS: &[&str] = &[
    "",
    "에서",
    "에서는",
    "에서도",
    "에게",
    "에게서",
    "한테",
    "으로",
    "으로는",
    "로",
    "로는",
    "로도",
    "에",
    "에는",
    "에도",
    "의",
    "은",
    "는",
    "이",
    "가",
    "을",
    "를",
    "도",
    "만",
    "랑",
    "이랑",
    "과",
    "와",
    "하고",
    "처럼",
    "까지",
    "부터",
    "보다",
    "이나",
    "나",
    "이고",
    "고",
    "이에요",
    "예요",
    "이죠",
    "죠",
    "이요",
    "요",
    "이야",
    "야",
    "쪽",
    "은요",
    "는요",
    "이랑은",
    "랑은",
    "같은",
    "말고",
];

/// 경계는 ASCII로 봐요: 한국어 문서는 "OpenRouter로"처럼 조사를 붙여 써서, 유니코드 경계로는 용어를 놓쳐요.
static DOC_WORD: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?-u:\b)[A-Z][A-Za-z0-9\-]{2,}(?-u:\b)|(?-u:\b)[a-z][a-z0-9]*-[a-z0-9\-]+(?-u:\b)")
        .unwrap()
});
static DECL: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"\b(?:struct|enum|class|protocol|trait|fn|func|const|static|actor)\s+([A-Za-z_][A-Za-z0-9_]{3,})")
        .unwrap()
});
static IDENT: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"[A-Za-z_][A-Za-z0-9_]{3,}").unwrap());
static CODE_SHAPE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"[a-z][A-Z]|_|^[A-Z][a-z]+[A-Z]").unwrap());
static LATIN_HEAD: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^[A-Za-z][A-Za-z0-9_\-.]*").unwrap());
/// Linear 이슈 번호("BEA-60")와 "D-90" 같은 번호는 용어가 아니에요.
static ISSUE_ID: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^[A-Z]+-[0-9]+$").unwrap());
static HANGUL_HEAD: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^[가-힣]+").unwrap());

/// 볼 수 있는 조각(GitHub 코드·문서·이슈, 노션·Linear·Drive·Dropbox 문서, 이 Mac의 노트)에서 팀 용어를 뽑아요. 문서에 자주 나오는
/// 제품·도구 이름과, 코드에 선언된 이름 중 자주 나오는 것이에요. 순서는 많이 나온 순, 같으면 먼저 나온 순이에요.
pub fn extract(chunks: &[Chunk], allowed: &[bool]) -> Vec<String> {
    let seen: Vec<&Chunk> = chunks
        .iter()
        .zip(allowed.iter().chain(std::iter::repeat(&true)))
        .filter(|(c, ok)| {
            **ok && matches!(
                c.src.as_str(),
                "code"
                    | "docs"
                    | "github"
                    | "notion"
                    | "linear"
                    | "gdrive"
                    | "dropbox"
                    | "jira"
                    | "confluence"
                    | "asana"
                    | "notes"
            )
        })
        .map(|(c, _)| c)
        .collect();
    let generic: HashSet<&str> = GENERIC.iter().copied().collect();

    let mut doc = Counted::default();
    for c in seen.iter().filter(|c| c.src != "code") {
        let words: Vec<&str> = DOC_WORD.find_iter(&c.text).map(|m| m.as_str()).collect();
        doc.add_once(words);
    }
    let mut out: Vec<String> = doc
        .top(DOC_MIN)
        .into_iter()
        .filter(|t| {
            !generic.contains(t.as_str())
                && !LOANWORDS.contains(&t.to_lowercase().as_str())
                && !ISSUE_ID.is_match(t)
        })
        .take(DOC_TERMS)
        .collect();

    let decl: HashSet<&str> = seen
        .iter()
        .filter(|c| c.src == "code")
        .flat_map(|c| {
            DECL.captures_iter(&c.text)
                .map(|m| m.get(1).unwrap().as_str())
        })
        .collect();
    let mut code = Counted::default();
    for c in &seen {
        let words: Vec<&str> = IDENT
            .find_iter(&c.text)
            .map(|m| m.as_str())
            .filter(|w| decl.contains(w))
            .collect();
        code.add_once(words);
    }
    let skip = ["fake", "test", "to_", "as_", "is_", "into_", "from_"];
    let code_terms = code
        .top(CODE_MIN)
        .into_iter()
        .filter(|t| CODE_SHAPE.is_match(t) && !skip.iter().any(|s| t.to_lowercase().starts_with(s)))
        .take(CODE_TERMS);
    for t in code_terms {
        if !out.contains(&t) {
            out.push(t);
        }
    }
    out
}

/// 조각마다 한 번씩 센 수예요(먼저 본 순서를 기억해요).
#[derive(Default)]
struct Counted {
    count: HashMap<String, (usize, usize)>,
}

impl Counted {
    fn add_once<'a>(&mut self, words: impl IntoIterator<Item = &'a str>) {
        let mut once = HashSet::new();
        for w in words {
            if once.insert(w) {
                let next = self.count.len();
                self.count.entry(w.to_string()).or_insert((0, next)).0 += 1;
            }
        }
    }

    fn top(&self, min: usize) -> Vec<String> {
        let mut v: Vec<(&String, &(usize, usize))> =
            self.count.iter().filter(|(_, (n, _))| *n >= min).collect();
        v.sort_by(|a, b| b.1.0.cmp(&a.1.0).then(a.1.1.cmp(&b.1.1)));
        v.into_iter().map(|(t, _)| t.clone()).collect()
    }
}

/// 용어 → 한글 표기 목록이에요. 이 Mac의 파일 하나(`term_readings.json`)에 두고, 새 용어만 모델에 물어요.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Readings(pub HashMap<String, Vec<String>>);

impl Readings {
    pub fn load(path: &Path) -> Self {
        let Ok(text) = std::fs::read_to_string(path) else {
            return Self::default();
        };
        let Ok(Value::Object(m)) = serde_json::from_str::<Value>(&text) else {
            return Self::default();
        };
        Self(
            m.into_iter()
                .map(|(k, v)| {
                    let rs = v
                        .as_array()
                        .map(|a| {
                            a.iter()
                                .filter_map(|x| x.as_str().map(str::to_string))
                                .collect()
                        })
                        .unwrap_or_default();
                    (k, rs)
                })
                .collect(),
        )
    }

    /// 임시 파일에 쓰고 바꿔 넣어요(쓰다 멈춰도 옛 파일이 남아요).
    pub fn save(&self, path: &Path) -> std::io::Result<()> {
        let mut keys: Vec<&String> = self.0.keys().collect();
        keys.sort();
        let mut m = Map::new();
        for k in keys {
            m.insert(k.clone(), Value::from(self.0[k].clone()));
        }
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        let tmp = PathBuf::from(format!("{}.tmp", path.display()));
        std::fs::write(
            &tmp,
            serde_json::to_string_pretty(&Value::Object(m)).unwrap_or_default(),
        )?;
        std::fs::rename(tmp, path)
    }

    pub fn missing(&self, terms: &[String]) -> Vec<String> {
        terms
            .iter()
            .filter(|t| !self.0.contains_key(*t))
            .cloned()
            .collect()
    }

    /// 모델 답(JSON 객체 글)을 읽어 `asked`에 든 용어만 더해요. 한글이 든 2자 넘는 표기만 받아요. 답에 없던
    /// 용어도 빈 목록으로 남겨서 다시 묻지 않아요. 읽지 못한 답이면 아무것도 더하지 않고 false예요.
    pub fn merge_reply(&mut self, asked: &[String], reply: &str) -> bool {
        let (Some(a), Some(b)) = (reply.find('{'), reply.rfind('}')) else {
            return false;
        };
        let Ok(Value::Object(m)) = serde_json::from_str::<Value>(&reply[a..=b]) else {
            return false;
        };
        for t in asked {
            let rs: Vec<String> = m
                .get(t)
                .and_then(Value::as_array)
                .map(|a| {
                    a.iter()
                        .filter_map(Value::as_str)
                        .map(str::trim)
                        .filter(|r| r.chars().count() >= 2 && r.chars().any(is_hangul))
                        .map(str::to_string)
                        .collect()
                })
                .unwrap_or_default();
            self.0.insert(t.clone(), rs);
        }
        true
    }
}

/// 표기를 만드는 모델 요청 글이에요. 용어는 정규식으로 뽑은 이름이라 지시를 품을 수 없어요.
#[cfg(feature = "local-prompts")]
pub const READINGS_PROMPT: &str = "아래 개발 용어 각각을 한국 개발자가 회의에서 말할 때 한글로 받아 적힐 법한 표기 1–3개로 적어요. \
     예: OpenRouter → [\"오픈라우터\", \"오픈 라우터\"], bge-m3 → [\"비지이엠쓰리\", \"bge 엠쓰리\"]. \
     흔한 한국어 낱말과 똑같은 표기(예: 다시, 타고, 이미)는 넣지 말아요. \
     한국에서 한글 이름이 그대로 굳어 쓰이는 이름(예: 노션, 슬랙, 깃허브)은 빈 목록 []으로 둬요. \
     JSON 객체 하나로만 답해요(키는 용어 그대로, 값은 표기 목록).\n\n{terms}";

/// 용어 표기를 묻는 프롬프트예요(`READINGS_PROMPT`, 배포 엔진에서는 중계 서버가 채워요, `prompts`).
pub fn readings(terms: &[String]) -> Prompt {
    Prompt {
        name: "readings",
        fields: vec![("terms", terms.join("\n"))],
    }
}

/// 다 채운 글이에요(`tacie-engine fix-terms`가 내보여요).
#[cfg(feature = "local-prompts")]
pub fn readings_prompt(terms: &[String]) -> String {
    readings(terms).text()
}

/// 한 번에 물을 용어 수예요(답이 중계 서버의 출력 한도 4096토큰 안에 들어가게).
pub const READINGS_BATCH: usize = 50;

fn is_hangul(c: char) -> bool {
    ('가'..='힣').contains(&c)
}

/// 한글 음절을 초성·중성·종성 번호로 풀어요(자모 단위로 비교하려고). 한글이 아닌 글자는 그대로 둬요.
fn jamo(s: &str) -> Vec<u32> {
    let mut out = Vec::new();
    for c in s.chars() {
        if is_hangul(c) {
            let n = c as u32 - 0xAC00;
            out.push(1000 + n / 588);
            out.push(2000 + (n % 588) / 28);
            if !n.is_multiple_of(28) {
                out.push(3000 + n % 28);
            }
        } else {
            out.push(c as u32);
        }
    }
    out
}

fn lev<T: PartialEq>(a: &[T], b: &[T]) -> usize {
    let mut prev: Vec<usize> = (0..=b.len()).collect();
    for (i, x) in a.iter().enumerate() {
        let mut cur = vec![i + 1; b.len() + 1];
        for (j, y) in b.iter().enumerate() {
            cur[j + 1] = (prev[j + 1] + 1)
                .min(cur[j] + 1)
                .min(prev[j] + usize::from(x != y));
        }
        prev = cur;
    }
    prev[b.len()]
}

/// 가까운 표기로 보인 말이 사실 "표기의 앞부분(두 음절 이상) + 조사"인지 봐요. "서비스에서"는 ServiceError의
/// 표기 "서비스에러"와 자모 하나 차이지만 "서비스" + "에서"예요(2026-11-17, 한국어 발표 받아쓰기 시험).
fn is_shorter_word_with_particle(word: &str, reading: &[u32]) -> bool {
    let chars: Vec<(usize, char)> = word.char_indices().collect();
    (2..chars.len()).any(|k| {
        let (b, _) = chars[k];
        let stem = jamo(&word[..b]);
        TAILS.contains(&&word[b..]) && reading.starts_with(&stem)
    })
}

fn latin_key(s: &str) -> String {
    s.chars()
        .filter(|c| !matches!(c, '-' | '_' | '.' | ' '))
        .flat_map(char::to_lowercase)
        .collect()
}

/// 뽑은 용어와 그 표기로 받아쓴 글을 고쳐요. 용어가 없으면 아무것도 바꾸지 않아요.
#[derive(Debug, Default)]
pub struct Terms {
    pub list: Vec<String>,
    /// (영문 비교 키, 용어)예요.
    latin: Vec<(Vec<char>, String)>,
    /// (표기 자모, 표기 음절 수, 용어)예요. 긴 표기부터예요.
    hangul: Vec<(Vec<u32>, usize, String)>,
}

impl Terms {
    pub fn new(list: Vec<String>, readings: &Readings) -> Self {
        let latin = list
            .iter()
            .map(|t| (latin_key(t).chars().collect::<Vec<_>>(), t.clone()))
            .filter(|(k, _)| k.len() >= 3)
            .collect();
        let mut hangul: Vec<(Vec<u32>, usize, String)> = list
            .iter()
            .flat_map(|t| {
                readings
                    .0
                    .get(t)
                    .into_iter()
                    .flatten()
                    .filter_map(move |r| {
                        let r: String = r.chars().filter(|c| !c.is_whitespace()).collect();
                        (r.chars().all(is_hangul) && r.chars().count() >= 2)
                            .then(|| (jamo(&r), r.chars().count(), t.clone()))
                    })
            })
            .collect();
        hangul.sort_by_key(|h| std::cmp::Reverse(h.1));
        Self {
            list,
            latin,
            hangul,
        }
    }

    pub fn is_empty(&self) -> bool {
        self.list.is_empty()
    }

    /// 받아쓴 글을 고쳐요. 띄어쓰기로 나눈 말 1–3개를 한 덩어리로 보고, 덩어리 앞부분이 용어(영문으로 비슷하게,
    /// 또는 한글 표기로)이고 뒤에 조사나 문장 부호만 남으면 용어로 바꿔요. 단어 시각(`words`)은 말 수가 글과
    /// 같을 때만 같이 합쳐요(앱이 녹음에서 말 하나씩 밝히는 데 써요). 다르면 그대로 둬요.
    pub fn fix(&self, text: &str, words: &[SpokenWord]) -> (String, Vec<SpokenWord>) {
        if self.is_empty() || text.trim().is_empty() {
            return (text.to_string(), words.to_vec());
        }
        let toks: Vec<&str> = text.split(' ').collect();
        let aligned =
            words.len() == toks.iter().filter(|t| !t.is_empty()).count() && !words.is_empty();
        let mut out: Vec<String> = Vec::new();
        let mut out_words: Vec<SpokenWord> = Vec::new();
        let mut wi = 0;
        let mut i = 0;
        while i < toks.len() {
            let mut hit = None;
            for n in (1..=3).rev() {
                if i + n > toks.len() || toks[i..i + n].iter().any(|t| t.is_empty()) {
                    continue;
                }
                if let Some(rep) = self.replace_span(&toks[i..i + n]) {
                    hit = Some((n, rep));
                    break;
                }
            }
            let (n, piece) = hit.unwrap_or((1, toks[i].to_string()));
            if aligned && !toks[i].is_empty() {
                let span = &words[wi..wi + n];
                out_words.push(SpokenWord {
                    text: if n == 1 && piece == toks[i] {
                        span[0].text.clone()
                    } else {
                        piece.clone()
                    },
                    start: span[0].start,
                    end: span[n - 1].end,
                });
                wi += n;
            }
            out.push(piece);
            i += n;
        }
        let words = if aligned { out_words } else { words.to_vec() };
        (out.join(" "), words)
    }

    /// 말 덩어리 하나를 용어로 바꾼 글이에요. 바꿀 게 없거나 바꿔도 같으면 None이에요.
    fn replace_span(&self, toks: &[&str]) -> Option<String> {
        let joined: String = toks.concat();
        // 끝 문장 부호는 떼었다가 다시 붙여요.
        let body = joined.trim_end_matches(['?', '.', '!', ',', '~']);
        let punct = &joined[body.len()..];
        let last_from = joined.len() - toks.last()?.len();
        let reaches_last = |cut: usize| toks.len() == 1 || cut > last_from;

        if let Some(m) = LATIN_HEAD.find(body) {
            let head = m.as_str().trim_end_matches(['-', '.']);
            let key: Vec<char> = latin_key(head).chars().collect();
            let tail = &body[head.len()..];
            if key.len() >= 3 && TAILS.contains(&tail) && reaches_last(head.len()) {
                let best = self
                    .latin
                    .iter()
                    .map(|(k, t)| (lev(&key, k), k.len(), t))
                    .min_by_key(|(d, _, _)| *d)?;
                let (d, len, term) = best;
                let rep = format!("{term}{tail}{punct}");
                // 띄어 쓴 덩어리는 영문이 모두 한 용어일 때만 합쳐요("Open Router" → OpenRouter).
                // 여섯 글자 미만 용어는 철자가 같을 때만이에요: 한 글자 차이면 흔한 낱말과 겹쳐요("was" → WAV,
                // "grant" → Graft, "Code" → Xcode, "Name" → CNAME, 2026-11-17 한국어 발표 받아쓰기 시험).
                // 숫자가 다르면 판 번호라 바꾸지 않아요("OAuth2" → OAuth).
                let allowed = if len < 6 { 0 } else { len / 6 };
                let digits =
                    |k: &[char]| k.iter().filter(|c| c.is_ascii_digit()).collect::<String>();
                let same_digits = latin_key(term).chars().collect::<Vec<_>>();
                if d <= allowed && digits(&key) == digits(&same_digits) && rep != toks.join(" ") {
                    return Some(rep);
                }
            }
        }

        let head = HANGUL_HEAD.find(body)?.as_str();
        let head_syl = head.chars().count();
        // 모든 표기 중 가장 가까운 것을 골라요(같으면 긴 표기). 먼저 걸린 걸 고르면 "인덱스 키퍼"가 비슷한
        // "인덱스 디어"(index_dir)로 바뀌어요.
        let mut best: Option<(usize, usize, String)> = None;
        for (rj, syl, term) in &self.hangul {
            for cut in [syl.saturating_sub(1), *syl, syl + 1] {
                if cut == 0 || cut > head_syl {
                    continue;
                }
                let bytes = head.char_indices().nth(cut).map_or(head.len(), |(b, _)| b);
                let tail = &body[bytes..];
                if !TAILS.contains(&tail) || !reaches_last(bytes) {
                    continue;
                }
                let cj = jamo(&head[..bytes]);
                let d = lev(&cj, rj);
                let ok = if d > 0
                    && (HANGUL_LOANWORDS.contains(&&head[..bytes])
                        || is_shorter_word_with_particle(&head[..bytes], rj))
                {
                    false
                } else if *syl <= 2 {
                    d == 0
                } else {
                    d <= (rj.len() / 6).max(1)
                };
                if ok
                    && best.as_ref().is_none_or(|(bd, bs, _)| {
                        (d, std::cmp::Reverse(*syl)) < (*bd, std::cmp::Reverse(*bs))
                    })
                {
                    best = Some((d, *syl, format!("{term}{tail}{punct}")));
                }
            }
        }
        best.map(|(_, _, rep)| rep)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn terms() -> Terms {
        let list: Vec<String> = [
            "OpenRouter",
            "PostHog",
            "Tacie",
            "PASSAGE_TOKENS",
            "ServiceError",
            "SSM",
        ]
        .iter()
        .map(|s| s.to_string())
        .collect();
        let mut r = Readings::default();
        r.0.insert(
            "OpenRouter".into(),
            vec!["오픈라우터".into(), "오픈 라우터".into()],
        );
        r.0.insert("PostHog".into(), vec!["포스트호그".into()]);
        r.0.insert("Tacie".into(), vec!["테이시".into(), "타시".into()]);
        r.0.insert(
            "ServiceError".into(),
            vec!["서비스에러".into(), "서비스 에러".into()],
        );
        r.0.insert("PASSAGE_TOKENS".into(), vec!["패시지토큰즈".into()]);
        Terms::new(list, &r)
    }

    fn fix(s: &str) -> String {
        terms().fix(s, &[]).0
    }

    #[test]
    fn hangul_reading_becomes_the_term_and_keeps_the_particle() {
        assert_eq!(
            fix("오픈라우터에서 보낼 때 맞죠?"),
            "OpenRouter에서 보낼 때 맞죠?"
        );
        assert_eq!(
            fix("포스트호그 이벤트에 넣으면 안 돼요"),
            "PostHog 이벤트에 넣으면 안 돼요"
        );
        assert_eq!(fix("오픈 라우터로 보내요."), "OpenRouter로 보내요.");
        assert_eq!(
            fix("서비스 에러는 뭐가 달라요?"),
            "ServiceError는 뭐가 달라요?"
        );
    }

    #[test]
    fn close_spelling_is_caught_with_the_jamo_distance() {
        assert_eq!(fix("오픈아우터에서 보내요"), "OpenRouter에서 보내요");
        assert_eq!(fix("패시지 토큰즈 값을 왜"), "PASSAGE_TOKENS 값을 왜");
    }

    #[test]
    fn the_closest_reading_wins_over_the_first_close_one() {
        let list: Vec<String> = ["index_dir", "IndexKeeper"]
            .iter()
            .map(|s| s.to_string())
            .collect();
        let mut r = Readings::default();
        r.0.insert("index_dir".into(), vec!["인덱스 디어".into()]);
        r.0.insert("IndexKeeper".into(), vec!["인덱스 키퍼".into()]);
        let t = Terms::new(list, &r);
        assert_eq!(
            t.fix("인덱스 키퍼가 동기화 끝난 걸", &[]).0,
            "IndexKeeper가 동기화 끝난 걸"
        );
    }

    #[test]
    fn latin_written_close_to_a_term_is_fixed() {
        assert_eq!(
            fix("Passage tokens 값을 왜 그렇게 정했어요?"),
            "PASSAGE_TOKENS 값을 왜 그렇게 정했어요?"
        );
        assert_eq!(fix("Open Router에서 보내요"), "OpenRouter에서 보내요");
        assert_eq!(fix("Posthog 이벤트"), "PostHog 이벤트");
    }

    #[test]
    fn common_words_and_other_endings_are_left_alone() {
        // 두 음절 표기는 똑같을 때만: "다시"는 "타시"가 아니에요.
        assert_eq!(fix("다시 임베딩 안 하죠?"), "다시 임베딩 안 하죠?");
        // 표기 뒤에 조사가 아닌 말이 붙으면 그대로예요.
        assert_eq!(fix("타시겠어요?"), "타시겠어요?");
        assert_eq!(
            fix("지난번에 화자 분리 기준 왜 바꿨는지 기억나세요?"),
            "지난번에 화자 분리 기준 왜 바꿨는지 기억나세요?"
        );
        assert_eq!(
            fix("이 버그 지금 누가 보고 있어요?"),
            "이 버그 지금 누가 보고 있어요?"
        );
        // 짧은 영어 낱말은 용어와 멀어요.
        assert_eq!(fix("the API is down"), "the API is down");
    }

    #[test]
    fn a_shorter_word_with_a_particle_is_not_a_close_reading() {
        // 2026-11-17 한국어 발표: "메일 서비스에서"가 "서비스에러"와 자모 하나 차이라 "메일 ServiceError"가 됐어요.
        assert_eq!(
            fix("메일 서비스에서 가져와서 호출했던"),
            "메일 서비스에서 가져와서 호출했던"
        );
        assert_eq!(fix("서비스에러가 나요"), "ServiceError가 나요");
        assert_eq!(fix("서비스 에러가 나요"), "ServiceError가 나요");
    }

    #[test]
    fn short_english_words_and_version_numbers_stay() {
        // 2026-11-17 한국어 발표 받아쓰기: 한 글자 차이 영어 낱말과 판 번호가 다른 용어로 바뀌었어요.
        let list: Vec<String> = ["Graft", "Xcode", "CNAME", "OAuth", "OpenRouter"]
            .iter()
            .map(|s| s.to_string())
            .collect();
        let t = Terms::new(list, &Readings::default());
        let f = |s: &str| t.fix(s, &[]).0;
        assert_eq!(f("grant를 받아요"), "grant를 받아요");
        assert_eq!(f("VS Code에서는"), "VS Code에서는");
        assert_eq!(f("Name을 넣어요"), "Name을 넣어요");
        assert_eq!(f("OAuth2 로그인"), "OAuth2 로그인");
        // 긴 용어는 한 글자 차이를 여전히 고쳐요.
        assert_eq!(f("OpenRauter에서"), "OpenRouter에서");
        assert_eq!(f("oauth 로그인"), "OAuth 로그인");
    }

    #[test]
    fn a_common_loanword_close_to_a_reading_stays_hangul() {
        // 예: "클래스 101"이 Glass의 표기 "글래스"와 자모 하나 차이라 "Glass 101"이 됐어요.
        let list: Vec<String> = ["Glass", "Dataset"].iter().map(|s| s.to_string()).collect();
        let mut r = Readings::default();
        r.0.insert("Glass".into(), vec!["글래스".into()]);
        r.0.insert("Dataset".into(), vec!["데이터셋".into()]);
        let t = Terms::new(list, &r);
        assert_eq!(t.fix("클래스 101이 이제", &[]).0, "클래스 101이 이제");
        assert_eq!(t.fix("데이터를 봐요", &[]).0, "데이터를 봐요");
        // 표기와 똑같으면 여전히 용어로 바꿔요.
        assert_eq!(t.fix("글래스 효과를 써요", &[]).0, "Glass 효과를 써요");
        assert_eq!(t.fix("데이터셋을 봐요", &[]).0, "Dataset을 봐요");
    }

    #[test]
    fn short_english_words_one_letter_off_a_short_term_stay() {
        // 2026-11-11 실제 회의: "the sync job was failing"이 용어 WAV 때문에 "WAV failing"이 됐어요.
        // 서너 글자 용어는 한 글자만 달라도 흔한 낱말(was, has, the)과 겹쳐서, 철자가 같을 때만 바꿔요.
        let list: Vec<String> = ["WAV", "SSE", "Jev"]
            .iter()
            .map(|s| s.to_string())
            .collect();
        let t = Terms::new(list, &Readings::default());
        let f = |s: &str| t.fix(s, &[]).0;
        assert_eq!(f("the sync job was failing"), "the sync job was failing");
        assert_eq!(f("she said yes"), "she said yes");
        assert_eq!(f("Jen sent it"), "Jen sent it");
        // 대소문자만 다르면 같은 철자예요.
        assert_eq!(f("wav 파일로 보내요"), "WAV 파일로 보내요");
        assert_eq!(f("jev가 판단해요"), "Jev가 판단해요");
        // 다섯 글자 넘는 용어는 지금처럼 한 글자 틀려도 고쳐요.
        assert_eq!(fix("Posthg 이벤트"), "PostHog 이벤트");
    }

    #[test]
    fn exact_term_stays_and_empty_list_changes_nothing() {
        assert_eq!(fix("OpenRouter에서 SSM 값을"), "OpenRouter에서 SSM 값을");
        let none = Terms::default();
        assert_eq!(none.fix("오픈라우터에서", &[]).0, "오픈라우터에서");
    }

    #[test]
    fn words_merge_with_the_tokens_they_replace() {
        let w = |t: &str, s: f64, e: f64| SpokenWord {
            text: t.into(),
            start: s,
            end: e,
        };
        let words = vec![
            w("오픈", 0.0, 0.3),
            w("라우터로", 0.3, 0.8),
            w("보내요", 0.9, 1.2),
        ];
        let (text, out) = terms().fix("오픈 라우터로 보내요", &words);
        assert_eq!(text, "OpenRouter로 보내요");
        assert_eq!(
            out,
            vec![w("OpenRouter로", 0.0, 0.8), w("보내요", 0.9, 1.2)]
        );
        // 말 수가 다르면 단어는 그대로예요.
        let (_, kept) = terms().fix("오픈 라우터로 보내요", &words[..2]);
        assert_eq!(kept, words[..2].to_vec());
    }

    #[test]
    fn readings_reply_keeps_only_asked_terms_with_hangul() {
        let mut r = Readings::default();
        let asked = vec!["OpenRouter".to_string(), "SSM".to_string()];
        let ok = r.merge_reply(
            &asked,
            "여기요:\n{\"OpenRouter\": [\"오픈라우터\", \"OR\", \"오\"], \"Other\": [\"아더\"]}",
        );
        assert!(ok);
        assert_eq!(r.0["OpenRouter"], vec!["오픈라우터".to_string()]);
        assert_eq!(r.0["SSM"], Vec::<String>::new());
        assert!(!r.0.contains_key("Other"));
        assert!(!r.merge_reply(&asked, "모르겠어요"));
    }

    #[test]
    fn extract_reads_allowed_github_and_document_chunks() {
        let ch = |src: &str, text: &str| Chunk {
            src: src.into(),
            path: "p".into(),
            line: 1,
            text: text.into(),
            title: None,
            url: None,
            acl: None,
            extra: Map::new(),
        };
        let mut chunks = Vec::new();
        for _ in 0..5 {
            chunks.push(ch("docs", "OpenRouter와 PostHog를 써요"));
        }
        for _ in 0..6 {
            chunks.push(ch("docs", "Data와 Domain, Payfold 결제"));
        }
        for _ in 0..9 {
            chunks.push(ch(
                "code",
                "struct MeetingModel {} fn load_state() {} let x = MeetingModel; load_state();",
            ));
        }
        for _ in 0..5 {
            chunks.push(ch("notion", "LaunchPlan 일정"));
            chunks.push(ch("linear", "PayfoldCheckout 버그 BEA-60"));
            chunks.push(ch("gdrive", "QuarterDeck 자료"));
            chunks.push(ch("slack", "ChatOnly 얘기"));
        }
        for _ in 0..9 {
            chunks.push(ch("notion", "SecretPlan SecretPlan"));
        }
        chunks.push(ch("docs", "HiddenThing"));
        let mut allowed = vec![true; chunks.len()];
        // 볼 수 없는 노션 조각의 이름도 넣지 않아요.
        let n = allowed.len();
        for a in allowed.iter_mut().skip(n - 10).take(9) {
            *a = false;
        }
        // 볼 수 없는 문서 조각의 이름은 넣지 않아요.
        for a in allowed.iter_mut().take(5) {
            *a = false;
        }
        let t = extract(&chunks, &allowed);
        assert!(t.contains(&"MeetingModel".to_string()));
        assert!(t.contains(&"load_state".to_string()));
        assert!(!t.contains(&"OpenRouter".to_string()));
        assert!(!t.iter().any(|x| x.contains("Secret")));
        for name in ["LaunchPlan", "PayfoldCheckout", "QuarterDeck"] {
            assert!(t.contains(&name.to_string()), "{name} missing from {t:?}");
        }
        assert!(!t.contains(&"BEA-60".to_string()));
        // 한글로 말하는 흔한 외래어는 용어가 아니에요. 이름은 남아요.
        assert!(!t.contains(&"Data".to_string()) && !t.contains(&"Domain".to_string()));
        assert!(t.contains(&"Payfold".to_string()));
        // 슬랙 대화는 넣지 않아요.
        assert!(!t.contains(&"ChatOnly".to_string()));
    }

    #[test]
    fn extract_reads_dropbox_chunks() {
        let chunks: Vec<Chunk> = (0..5)
            .map(|_| Chunk {
                src: "dropbox".into(),
                path: "p".into(),
                line: 1,
                text: "QuarterBudget 자료".into(),
                title: None,
                url: None,
                acl: None,
                extra: Map::new(),
            })
            .collect();
        let t = extract(&chunks, &vec![true; chunks.len()]);
        assert!(t.contains(&"QuarterBudget".to_string()), "{t:?}");
    }
}
