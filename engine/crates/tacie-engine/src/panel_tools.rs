//! Tool descriptions for the Tacie AI panel harness (docs/38 3-2절). Tool names and descriptions are prompt text
//! too, so the shipped app's engine does not have them (`local-prompts`); the relay server fills them in by set
//! name from `tools` in `prompts.json`. The engine sends only the name, like `tools: "panel"`.
//!
//! There are four sets. `panel` covers all meetings (no transcripts, so no `transcript_window`), and
//! `panel_meeting_edit` is a chat started from a meeting page (the app attaches those meetings' transcripts to the
//! request, 3-3절). `panel_notes_edit` is the page of a meeting whose transcript is not on this Mac (an iPhone
//! meeting). These two have write tools (12절). Write tools only propose; the user makes the change on the app's
//! confirm card. The iPhone engine uses `panel_iphone`, `panel_iphone_meeting` and `panel_iphone_notes` in the same
//! places (`iphone_sets`). The iPhone all-meetings set (`panel_iphone`) has `open_meeting` instead of write tools:
//! it picks the one meeting to fix and passes it on, and the app attaches that meeting's transcript and sends the
//! same question again with the meeting set (docs/40 12절). `panel_meeting` is the meeting page set from before
//! write tools. Shipped apps' engines still send this name, so it stays as is: even if the relay server is deployed
//! first, those apps do not get write tools without a confirm card.

use serde_json::{Value, json};

/// 묶음마다 OpenRouter `tools` 배열이에요.
#[cfg(feature = "local-prompts")]
pub fn sets() -> Value {
    let all = vec![search(), list_meetings(), filter_ledger(), prepare_meeting()];
    let write = vec![meeting_notes(), find_words(), fix_words(), edit_ledger(), rename_title()];
    let mut meeting = all.clone();
    meeting.insert(3, transcript_window());
    meeting.extend(write.clone());
    // 화자는 대화 기록에만 있어요: 대화 기록이 없는 회의(iPhone)에는 없어요
    meeting.push(name_speaker());
    meeting.push(merge_speakers());
    meeting.push(move_lines());
    meeting.push(assign_lines());
    meeting.push(rewrite_notes());
    meeting.push(change_kind());
    let mut notes = all.clone();
    notes.extend(write);
    let mut read = all.clone();
    read.insert(3, transcript_window());
    let (iphone, iphone_meeting, iphone_notes) = iphone_sets();
    json!({"panel": all, "panel_meeting": read, "panel_meeting_edit": meeting, "panel_notes_edit": notes,
           "panel_iphone": iphone, "panel_iphone_meeting": iphone_meeting, "panel_iphone_notes": iphone_notes})
}

/// iPhone 묶음 셋이에요(docs/40 4-5절): 모든 회의, 대화 기록이 있는 회의 페이지, 대화 기록이 없는 회의 페이지예요. 찾기는
/// 이 iPhone의 지난 회의만이고 `wide`가 없어요. 화자 합치기와 사람 연결은 동기화되지 않아서 `merge_speakers`,
/// `move_lines`, `assign_lines`가 없고, iPhone의 회의 정리가 형식을 받지 않아서 `change_kind`도 없어요.
#[cfg(feature = "local-prompts")]
fn iphone_sets() -> (Vec<Value>, Vec<Value>, Vec<Value>) {
    let read = vec![search_iphone(), list_meetings(), filter_ledger(), prepare_meeting()];
    let write = vec![meeting_notes(), find_words(), fix_words(), edit_ledger(), rename_title()];
    let mut meeting = read.clone();
    meeting.insert(3, transcript_window());
    meeting.extend(write.clone());
    meeting.push(name_speaker());
    meeting.push(rewrite_notes());
    let mut notes = read.clone();
    notes.extend(write);
    // 모든 회의에서는 고치지 않고, 고칠 회의 하나를 앱에 넘겨요(docs/40 12절). 회의 묶음에는 없어서 넘김이 한 번뿐이에요.
    let mut all = read;
    all.push(open_meeting());
    (all, meeting, notes)
}

/// 모든 묶음에 나오는 도구 이름이에요. 중계 서버도 같은 이름만 받아요.
pub const NAMES: [&str; 17] = [
    "search",
    "list_meetings",
    "filter_ledger",
    "transcript_window",
    "prepare_meeting",
    "meeting_notes",
    "find_words",
    "fix_words",
    "edit_ledger",
    "name_speaker",
    "merge_speakers",
    "move_lines",
    "rewrite_notes",
    "change_kind",
    "rename_title",
    "assign_lines",
    "open_meeting",
];

#[cfg(feature = "local-prompts")]
fn tool(name: &str, description: &str, properties: Value, required: &[&str]) -> Value {
    json!({"type": "function", "function": {
        "name": name,
        "description": description,
        "parameters": {"type": "object", "properties": properties, "required": required,
                       "additionalProperties": false},
    }})
}

#[cfg(feature = "local-prompts")]
const DATE: &str = "YYYY-MM-DD, 그날 포함";

#[cfg(feature = "local-prompts")]
fn search() -> Value {
    tool(
        "search",
        "회의 정리와 연결한 서비스(GitHub, Slack, Notion, Linear, Jira 등)의 글을 뜻으로 찾아요. 주제나 사실을 물을 때 써요. \
         질문을 그대로 넣지 말고 찾을 내용을 짧은 검색어로 바꿔 넣어요. 다른 두 가지를 물으면 두 번 나눠 불러요. \
         날짜·종류·담당자로 거르는 질문에는 filter_ledger가 맞아요. 결과마다 [번호]가 붙고, 답에서 그 번호로 근거를 달아요.",
        json!({
            "query": {"type": "string", "description": "찾을 내용(짧은 검색어)"},
            "wide": {"type": "boolean", "description": "회의 페이지에서 시작한 대화에서, 그 회의 밖(모든 회의와 연결한 서비스)까지 찾을 때 true. 먼저 false로 찾아보고 없을 때 써요"},
        }),
        &["query"],
    )
}

/// iPhone의 `search`예요: 이 iPhone의 지난 회의 정리만 찾고, 회의 페이지 밖으로 넓히는 `wide`가 없어요.
#[cfg(feature = "local-prompts")]
fn search_iphone() -> Value {
    tool(
        "search",
        "이 iPhone의 지난 회의 정리(요약, 주제, 결정, 할 일)를 뜻으로 찾아요. 주제나 사실을 물을 때 써요. \
         질문을 그대로 넣지 말고 찾을 내용을 짧은 검색어로 바꿔 넣어요. 다른 두 가지를 물으면 두 번 나눠 불러요. \
         날짜·종류·담당자로 거르는 질문에는 filter_ledger가 맞아요. 결과마다 [번호]가 붙고, 답에서 그 번호로 근거를 달아요.",
        json!({"query": {"type": "string", "description": "찾을 내용(짧은 검색어)"}}),
        &["query"],
    )
}

#[cfg(feature = "local-prompts")]
fn list_meetings() -> Value {
    tool(
        "list_meetings",
        "회의 목록을 기간이나 제목·요약 글로 걸러요. 언제 무슨 회의가 있었는지, 어느 회의를 볼지 정할 때 써요. 결과는 회의 id, 제목, 날짜, 요약 첫 문장이에요.",
        json!({
            "from": {"type": "string", "description": DATE},
            "to": {"type": "string", "description": DATE},
            "text": {"type": "string", "description": "제목·요약에 들어 있을 말"},
        }),
        &[],
    )
}

#[cfg(feature = "local-prompts")]
fn filter_ledger() -> Value {
    tool(
        "filter_ledger",
        "회의 정리의 결정, 할 일, 남은 논의를 종류·담당자·끝남·기간·회의로 걸러요. \"이번 달 정한 것\", \"민호가 맡은 것\", \"아직 안 정한 것\"처럼 목록을 묻는 질문에 써요. \
         결과마다 [번호], 종류, 글, 담당자, 기한, 끝남, 회의 날짜와 제목, 이 결정이 바꾼 지난 결정이 있어요.",
        json!({
            "kinds": {"type": "array", "items": {"type": "string", "enum": ["decision", "commitment", "open_question"]},
                      "description": "decision=결정, commitment=할 일, open_question=남은 논의. 비우면 모두"},
            "owner": {"type": "string", "description": "담당자 이름의 일부"},
            "done": {"type": "boolean", "description": "할 일이 끝났는지. 비우면 모두"},
            "from": {"type": "string", "description": DATE},
            "to": {"type": "string", "description": DATE},
            "meeting_ids": {"type": "array", "items": {"type": "string"}, "description": "이 회의들만"},
            "text": {"type": "string", "description": "항목 글이나 회의 제목에 들어 있을 낱말(띄어 쓰면 모두 들어 있는 것만)"},
        }),
        &[],
    )
}

#[cfg(feature = "local-prompts")]
fn transcript_window() -> Value {
    tool(
        "transcript_window",
        "회의 하나의 대화 기록에서 한 구간을 읽어요. 누가 무슨 말을 했는지, 왜 그렇게 정했는지, 누가 반대했는지처럼 회의 정리에 없는 말을 확인할 때 써요. \
         find에 낱말 몇 개를 띄어 넣으면 그 낱말이 많이 나온 발화(말한 사람 이름도 세요) 세 곳의 앞뒤를, key에 발화 key를 넣으면 그 발화 앞뒤를 돌려줘요.",
        json!({
            "meeting_id": {"type": "string"},
            "find": {"type": "string", "description": "찾을 낱말 여러 개, 띄어 써요(예: \"민호 환불 통지\")"},
            "key": {"type": "string", "description": "발화 key(예: e0:s1u12)"},
            "around": {"type": "integer", "description": "앞뒤로 읽을 발화 수(기본 6, 최대 20)"},
            "speaker": {"type": "string", "description": "화자 번호나 이름(예: \"화자 5\"): 그 화자의 발화마다 앞뒤 두 줄을 화자 번호와 함께 돌려줘요. 잘못 나뉜 화자의 말이 누구 말인지 볼 때 써요"},
        }),
        &["meeting_id"],
    )
}

#[cfg(feature = "local-prompts")]
fn prepare_meeting() -> Value {
    tool(
        "prepare_meeting",
        "회의 하나와 그 회의로 이어진 앞 회의들을 묶어 다음 회의 준비를 해요. 아직 안 정한 것, 안 끝난 할 일, 관련 자료를 돌려줘요. \
         \"다음 회의 준비\", \"들어가기 전에 챙길 것\"을 물을 때 써요.",
        json!({"meeting_id": {"type": "string"}}),
        &["meeting_id"],
    )
}

#[cfg(feature = "local-prompts")]
fn meeting_notes() -> Value {
    tool(
        "meeting_notes",
        "지금 보는 회의의 정리(제목, 요약, 주제, 장부)를 읽어요. 장부 항목마다 [번호]가 붙어요. \
         잘못 들린 말이 무엇인지 사용자가 말하지 않았을 때 정리에서 찾거나, edit_ledger에 넣을 항목 번호를 받을 때 써요.",
        json!({"meeting_id": {"type": "string", "description": "비우면 지금 보는 회의"}}),
        &[],
    )
}

#[cfg(feature = "local-prompts")]
fn find_words() -> Value {
    tool(
        "find_words",
        "고치기 전에, 지금 보는 회의의 대화 기록과 정리에서 글자가 그대로 나온 곳을 모두 찾아요. 곳마다 w번호, 자리, 앞뒤 글이 있어요. \
         사용자가 잘못 들린 말을 고쳐 달라고 할 때만 써요.",
        json!({"word": {"type": "string", "description": "잘못 들린 말, 글자 그대로(예: \"축소심사\")"}}),
        &["word"],
    )
}

#[cfg(feature = "local-prompts")]
fn fix_words() -> Value {
    tool(
        "fix_words",
        "받아쓰기가 잘못 들은 말이나 사용자가 바꾸고 싶은 표기(\"에이피아이\" → \"API\")를 맞는 말로 바꾸자고 사용자에게 제안해요. 바로 바뀌지 않고, 사용자가 확인 카드에서 고른 뒤 바뀌어요. \
         사용자가 이번 질문에서 고쳐 달라고 했을 때만 불러요. 먼저 find_words로 heard를 찾고, 찾은 곳마다 앞뒤 뜻을 읽어요. \
         같은 글자가 원래 맞게 쓰인 곳은 그 w번호를 skip에 넣어요. 뺄 곳이 없으면 skip을 빈 목록으로 넣어요. skip에 넣지 않은 곳은 모두 바뀌어요.",
        json!({
            "heard": {"type": "string", "description": "잘못 들린 말, 기록에 적힌 글자 그대로"},
            "meant": {"type": "string", "description": "맞는 말"},
            "skip": {"type": "array", "items": {"type": "string"}, "description": "바꾸지 않을 곳(원래 맞게 쓰인 곳)의 w번호(예: [\"w3\", \"w7\"]). 없으면 []"},
        }),
        &["heard", "meant", "skip"],
    )
}

#[cfg(feature = "local-prompts")]
fn edit_ledger() -> Value {
    tool(
        "edit_ledger",
        "지금 보는 회의의 장부(결정, 할 일, 남은 논의)를 고치자고 사용자에게 제안해요. 바로 바뀌지 않고, 사용자가 확인 카드에서 고른 뒤 바뀌어요. \
         사용자가 이번 질문에서 고쳐 달라고 했을 때만 불러요. 항목은 meeting_notes나 filter_ledger가 준 [번호]로 가리켜요. 항목의 종류(결정, 할 일)는 바꾸지 못해요.",
        json!({
            "changes": {"type": "array", "description": "고칠 항목들", "items": {"type": "object", "properties": {
                "item": {"type": "integer", "description": "항목의 [번호]"},
                "text": {"type": "string", "description": "새 글"},
                "owner": {"type": "string", "description": "새 담당자 이름"},
                "due": {"type": "string", "description": "새 기한, 사용자가 말한 그대로(예: \"5월 11일 오전\")"},
                "done": {"type": "boolean", "description": "할 일을 끝냄(true) 또는 안 끝남(false)"},
                "remove": {"type": "boolean", "description": "항목을 지울 때 true"},
            }, "required": ["item"], "additionalProperties": false}},
            "add": {"type": "array", "description": "새 할 일", "items": {"type": "object", "properties": {
                "text": {"type": "string"},
                "owner": {"type": "string"},
                "due": {"type": "string"},
            }, "required": ["text"], "additionalProperties": false}},
        }),
        &[],
    )
}

#[cfg(feature = "local-prompts")]
fn name_speaker() -> Value {
    tool(
        "name_speaker",
        "지금 보는 회의에서 화자(\"화자 1\" 같은 번호나 지금 붙은 이름)에게 이름을 붙이자고 사용자에게 제안해요. 바로 바뀌지 않고, 사용자가 확인 카드에서 고른 뒤 바뀌어요. \
         이름은 그 회의 전체(대화 기록, 요약, 주제, 장부, 담당자)에 같이 붙어요. 사용자가 이번 질문에서 화자 이름을 바꿔 달라고 했을 때만 불러요.",
        json!({
            "speaker": {"type": "string", "description": "대화 기록에 보이는 화자 그대로(예: \"화자 1\", \"지윤\")"},
            "name": {"type": "string", "description": "붙일 이름, 사용자가 말한 그대로"},
        }),
        &["speaker", "name"],
    )
}

#[cfg(feature = "local-prompts")]
fn merge_speakers() -> Value {
    tool(
        "merge_speakers",
        "화자 분리가 한 사람을 여러 화자로 나눴을 때, 그 화자들을 한 화자로 합치자고 사용자에게 제안해요. 바로 바뀌지 않고, 사용자가 확인 카드에서 고른 뒤 바뀌어요. \
         from 화자들의 대화 기록 줄이 모두 into 화자의 것이 되고 이름도 into를 따라요. 사용자가 이번 질문에서 화자를 합치거나 \"같은 사람\"이라고 했을 때만 불러요.",
        json!({
            "from": {"type": "array", "items": {"type": "string"}, "description": "합쳐서 없앨 화자들, 번호나 지금 이름(예: [\"화자 3\", \"화자 5\"])"},
            "into": {"type": "string", "description": "남길 화자, 번호나 지금 이름(예: \"서연\")"},
        }),
        &["from", "into"],
    )
}

#[cfg(feature = "local-prompts")]
fn move_lines() -> Value {
    tool(
        "move_lines",
        "대화 기록의 몇 줄만 다른 화자가 한 말로 옮기자고 사용자에게 제안해요. 화자 전체가 잘못이면 merge_speakers를 써요. 바로 바뀌지 않고, 사용자가 확인 카드에서 고른 뒤 바뀌어요. \
         줄은 먼저 transcript_window로 그 말을 찾아 [번호]로 가리켜요. 사용자가 이번 질문에서 그 말을 다른 사람이 했다고 했을 때만 불러요.",
        json!({
            "lines": {"type": "array", "items": {"type": "integer"}, "description": "옮길 줄의 [번호](transcript_window 결과)"},
            "to": {"type": "string", "description": "그 말을 한 화자, 번호나 지금 이름(예: \"화자 2\", \"서연\"). 이 회의에 없는 이름이면 새 사람이에요"},
        }),
        &["lines", "to"],
    )
}

#[cfg(feature = "local-prompts")]
fn assign_lines() -> Value {
    tool(
        "assign_lines",
        "화자 분리가 몇 줄짜리 화자를 잘못 만들었을 때, 그 줄마다 실제로 말한 화자를 정하자고 사용자에게 제안해요. 줄마다 옮길 화자가 달라도 돼요. \
         먼저 transcript_window의 speaker로 그 화자의 줄과 앞뒤를 읽고, 문장이 이어지는지, 묻고 답하는 흐름, 부른 이름, 말투로 누구 말인지 정해요. \
         확실하지 않은 줄은 넣지 않아요. 바로 바뀌지 않고, 사용자가 확인 카드에서 줄마다 고른 뒤 바뀌어요.",
        json!({"moves": {"type": "array", "description": "옮길 줄들", "items": {"type": "object", "properties": {
            "line": {"type": "integer", "description": "줄의 [번호](transcript_window 결과)"},
            "to": {"type": "string", "description": "그 말을 한 화자, 이 회의 화자의 번호나 이름"},
        }, "required": ["line", "to"], "additionalProperties": false}}}),
        &["moves"],
    )
}

#[cfg(feature = "local-prompts")]
fn rewrite_notes() -> Value {
    tool(
        "rewrite_notes",
        "지금 보는 회의의 정리(제목, 요약, 주제, 장부)를 대화 기록으로 다시 쓰자고 사용자에게 제안해요. 사용자의 요청(더 자세히, 짧게, 결정 위주로 같은 것)을 함께 넘겨요. \
         바로 바뀌지 않고, 사용자가 확인 카드에서 고른 뒤 다시 써요. 사용자가 손으로 고친 장부 항목은 남아요. 사용자가 이번 질문에서 정리를 다시 써 달라거나 정리가 마음에 안 든다고 했을 때만 불러요.",
        json!({"request": {"type": "string", "description": "사용자가 원하는 것, 사용자의 말 그대로 짧게(예: \"요약을 더 자세히\"). 없으면 빈 글"}}),
        &["request"],
    )
}

#[cfg(feature = "local-prompts")]
fn change_kind() -> Value {
    tool(
        "change_kind",
        "회의 정리의 형식을 바꿔 다시 쓰자고 사용자에게 제안해요. meeting은 결정 사항·할 일·남은 논의, interview(인터뷰·대담)는 질문과 답·핵심 숫자·기억할 말, talk(강의·발표)는 내용 흐름·핵심 개념·기억할 점이에요. \
         바로 바뀌지 않고, 사용자가 확인 카드에서 고른 뒤 다시 써요. 사용자가 이번 질문에서 이 회의가 다른 종류라고 했을 때만 불러요.",
        json!({"kind": {"type": "string", "enum": ["meeting", "interview", "talk"], "description": "새 형식"}}),
        &["kind"],
    )
}

#[cfg(feature = "local-prompts")]
fn rename_title() -> Value {
    tool(
        "rename_title",
        "지금 보는 회의의 제목을 바꾸자고 사용자에게 제안해요. 바로 바뀌지 않고, 사용자가 확인 카드에서 고른 뒤 바뀌어요. 사용자가 이번 질문에서 제목을 바꿔 달라고 했을 때만 불러요.",
        json!({"title": {"type": "string", "description": "새 제목, 사용자가 말한 그대로"}}),
        &["title"],
    )
}

/// iPhone 모든 회의 대화에서 고쳐 달라는 회의 하나를 여는 도구예요(docs/40 12절). 엔진은 아무것도 바꾸지 않고 질문을
/// 끝내며 `done`의 `handoff`로 그 회의를 앱에 알려요. 앱이 그 회의의 대화 기록을 붙여 같은 질문을 다시 보내요.
#[cfg(feature = "local-prompts")]
fn open_meeting() -> Value {
    tool(
        "open_meeting",
        "찾은 회의 하나를 열어, 그 회의의 대화 기록과 정리로 고치기를 이어 가요. 사용자가 이번 질문에서 어느 회의의 화자 이름, 잘못 들린 말, 결정·할 일, 제목, 정리를 바꿔 달라고 했을 때만 불러요. \
         먼저 search나 list_meetings로 그 회의를 찾아 id를 넣어요. 맞는 회의가 둘 이상이면 부르지 말고 회의 이름으로 어느 회의인지 물어요. \
         부르면 이번 답은 여기서 끝나고, 그 회의에서 고치기 제안이 이어져요. 묻기만 하는 질문에는 부르지 않아요.",
        json!({"meeting_id": {"type": "string", "description": "고칠 회의의 id(search나 list_meetings 결과의 id)"}}),
        &["meeting_id"],
    )
}

#[cfg(all(test, feature = "local-prompts"))]
mod tests {
    use super::*;

    #[test]
    fn sets_use_only_the_known_names() {
        let s = sets();
        for set in ["panel", "panel_meeting", "panel_meeting_edit", "panel_notes_edit"] {
            for t in s[set].as_array().unwrap() {
                let n = t["function"]["name"].as_str().unwrap();
                assert!(NAMES.contains(&n), "{n}");
            }
        }
        let names = |set: &str| -> Vec<String> {
            s[set].as_array().unwrap().iter().map(|t| t["function"]["name"].as_str().unwrap().to_string()).collect()
        };
        assert!(!names("panel").contains(&"transcript_window".to_string()));
        assert!(names("panel_meeting_edit").contains(&"transcript_window".to_string()));
        assert!(!names("panel_notes_edit").contains(&"transcript_window".to_string()));
        assert!(names("panel_meeting_edit").contains(&"name_speaker".to_string()));
        assert!(names("panel_meeting_edit").contains(&"merge_speakers".to_string()));
        assert!(!names("panel_notes_edit").contains(&"merge_speakers".to_string()));
        for t in ["move_lines", "assign_lines", "rewrite_notes", "change_kind"] {
            assert!(names("panel_meeting_edit").contains(&t.to_string()), "{t}");
            assert!(!names("panel_notes_edit").contains(&t.to_string()), "{t}");
        }
        assert!(names("panel_notes_edit").contains(&"rename_title".to_string()));
        // the bundle the shipped engines send keeps no write tools
        for write in ["meeting_notes", "find_words", "fix_words", "edit_ledger", "name_speaker", "merge_speakers", "move_lines", "assign_lines", "rewrite_notes", "change_kind", "rename_title"] {
            assert!(!names("panel_meeting").contains(&write.to_string()), "{write}");
        }
        assert!(names("panel_meeting").contains(&"transcript_window".to_string()));
        assert!(!names("panel_notes_edit").contains(&"name_speaker".to_string()));
        assert!(!names("panel").contains(&"name_speaker".to_string()));
        for write in ["find_words", "fix_words", "edit_ledger"] {
            assert!(!names("panel").contains(&write.to_string()), "{write}");
            assert!(names("panel_meeting_edit").contains(&write.to_string()), "{write}");
            assert!(names("panel_notes_edit").contains(&write.to_string()), "{write}");
        }
    }

    const IPHONE_SETS: [&str; 3] = ["panel_iphone", "panel_iphone_meeting", "panel_iphone_notes"];

    #[test]
    fn iphone_sets_leave_out_mac_tools() {
        let s = sets();
        let names = |set: &str| -> Vec<String> {
            s[set].as_array().unwrap().iter().map(|t| t["function"]["name"].as_str().unwrap().to_string()).collect()
        };
        for set in IPHONE_SETS {
            for n in names(set) {
                assert!(NAMES.contains(&n.as_str()), "{set} {n}");
            }
            for mac in ["merge_speakers", "move_lines", "assign_lines", "change_kind"] {
                assert!(!names(set).contains(&mac.to_string()), "{set} {mac}");
            }
            for read in ["search", "list_meetings", "filter_ledger", "prepare_meeting"] {
                assert!(names(set).contains(&read.to_string()), "{set} {read}");
            }
            let search = s[set].as_array().unwrap().iter().find(|t| t["function"]["name"] == "search").unwrap();
            assert!(search["function"]["parameters"]["properties"].get("wide").is_none(), "{set}");
        }
        for t in ["name_speaker", "fix_words", "edit_ledger", "rename_title", "rewrite_notes", "transcript_window", "find_words", "meeting_notes"] {
            assert!(names("panel_iphone_meeting").contains(&t.to_string()), "{t}");
            assert!(!names("panel_iphone").contains(&t.to_string()), "{t}");
        }
        // 대화 기록이 없는 회의: Mac의 panel_notes_edit과 같은 규칙이에요
        for t in ["meeting_notes", "find_words", "fix_words", "edit_ledger", "rename_title"] {
            assert!(names("panel_iphone_notes").contains(&t.to_string()), "{t}");
        }
        for t in ["transcript_window", "name_speaker", "rewrite_notes"] {
            assert!(!names("panel_iphone_notes").contains(&t.to_string()), "{t}");
        }
        // 고칠 회의를 넘기는 도구는 모든 회의 묶음에만 있어요: 회의 묶음에서 다시 넘기지 않아요(docs/40 12절)
        assert!(names("panel_iphone").contains(&"open_meeting".to_string()));
        for set in ["panel_iphone_meeting", "panel_iphone_notes", "panel", "panel_meeting", "panel_meeting_edit", "panel_notes_edit"] {
            assert!(!names(set).contains(&"open_meeting".to_string()), "{set}");
        }
    }

    #[test]
    fn iphone_texts_name_no_services() {
        let s = sets();
        let banned = ["GitHub", "Slack", "Notion", "연결한 서비스", "Mac"];
        let mut texts = vec![
            crate::openrouter::PANEL_CHAT_IPHONE_PROMPT.to_string(),
            crate::prompts_en::PANEL_CHAT_IPHONE_PROMPT_EN.to_string(),
        ];
        for set in IPHONE_SETS {
            texts.push(s[set].to_string());
        }
        for t in &texts {
            for b in banned {
                assert!(!t.contains(b), "{b} in {}", t.chars().take(80).collect::<String>());
            }
        }
        // Mac 묶음은 그대로예요: search에 wide가 있어요
        let mac = s["panel"].as_array().unwrap().iter().find(|t| t["function"]["name"] == "search").unwrap();
        assert!(mac["function"]["parameters"]["properties"].get("wide").is_some());
    }
}
