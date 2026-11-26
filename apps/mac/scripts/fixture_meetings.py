"""Writes the main-window mock's sample meetings as SavedMeeting JSON, dated around today, for screenshots.

usage: uv run python apps/mac/scripts/fixture_meetings.py <folder> [--speakers]
--speakers adds an in-person meeting with speakers and names (the lines of the speaker mockups, mock2.swift).
--landing puts the landing page's meeting (site After section, 민호 · 지안 · 화자 3) in place of the 14:00 one.
then:  open -n apps/mac/build.noindex/Build/Products/Debug/Tacie.app --args --allow-capture --debug-control --meetings-dir <folder>
The content is made up; file names are not real.
"""
import json
import sys
from datetime import datetime, timedelta, timezone
from pathlib import Path

TODAY = datetime.now().astimezone().replace(hour=0, minute=0, second=0, microsecond=0)


def at(days_ago, h, m=0, s=0):
    return TODAY - timedelta(days=days_ago) + timedelta(hours=h, minutes=m, seconds=s)


def iso(d):
    return d.astimezone(timezone.utc).strftime("%Y-%m-%dT%H:%M:%SZ")


def card(cid, path, kind, title, n, url=None):
    return {"id": cid, "path": path, "kind": kind, "source": "repo" if kind in ("code", "docs") else "github",
            "lineStart": 1, "lineEnd": 20, "title": title, "snippet": "", "chunk": n,
            "githubUrl": url or f"https://github.com/example/repo/blob/dev/{path}"}


def meeting(start, minutes, mode, title, summary, transcript=None, **more):
    m = {"version": 1, "id": start.strftime("%Y%m%d-%H%M%S"), "mode": mode, "start": iso(start),
         "end": iso(start + timedelta(minutes=minutes)), "title": title, "titleEdited": False, "notes": "done",
         "summary": summary, "ledger": [], "questions": [], "transcript": transcript or [
             {"key": "e0:s1u1", "at": iso(start + timedelta(minutes=1)), "speaker": None, "text": "시작할게요.", "question": False}]}
    m.update(more)
    return m


def speakers_meeting():
    """Today 15:00, in person: 화자 1 named 민재, 화자 2 and 3 unnamed; 화자 3 asks and Tacie answers."""
    s = at(0, 15)
    lines = [
        (1, "화자 1", "자, 이번 주 배포 이야기부터 할게요.", False),
        (1.2, "화자 1", "목요일에 나가는 걸로 다들 괜찮죠? 금요일은 사람이 적어서 피하고 싶어요.", False),
        (2, "화자 2", "네, 저는 괜찮아요. QA는 수요일 오후까지 끝낼게요.", False),
        (2.4, "화자 3", "그런데 결제 실패하면 재시도 몇 번 해요?", True),
        (3, "화자 1", "세 번이었던 것 같은데, Tacie가 찾아 줬네요. 세 번 맞아요.", False),
        (3.2, "화자 1", "간격은 1분, 5분, 30분이고요.", False),
        (3.5, "화자 2", "그럼 공지는 제가 쓸게요.", False),
        (4, "화자 3", "좋아요. 다음은 알림 묶음이요.", False),
    ]
    transcript = [{"key": f"e0:s1u{i + 1}", "at": iso(s + timedelta(minutes=mm)), "speaker": None, "text": text,
                   "question": q, "voice": voice} for i, (mm, voice, text, q) in enumerate(lines)]
    return meeting(s, 34, "inperson", "목요일 배포와 결제 재시도", [
        "배포는 목요일로 정했어요. 금요일은 사람이 적어서 피해요.",
        "결제 재시도는 세 번(1분, 5분, 30분)이라는 걸 확인했어요."], transcript,
        voices=[{"key": "화자 1", "name": "민재"}],
        ledger=[{"kind": "commitment", "text": "배포 공지 쓰기", "owner": "화자 2", "due": "수요일까지", "done": False}],
        questions=[{"id": "v1", "question": "결제 실패하면 재시도 몇 번 해요?", "asker": None, "voice": "화자 3",
                    "at": iso(s + timedelta(minutes=2.4)), "answer": "세 번이에요. 1분, 5분, 30분 간격으로 다시 시도해요. [1]",
                    "cards": [card("c1", "src/payment/retry_policy.rs", "code", "retry_policy.rs", 1),
                              card("c2", "docs/결제 장애 대응.md", "docs", "결제 장애 대응", 2)],
                    "cited": [1], "unanswered": False}])


def landing_meeting():
    """Today 14:00, remote: the landing page's meeting page (site After section's 민호 and 지안, plus 화자 3 unnamed).
    지안 asks why retrying right away was dropped and Tacie answers; the notes show summary, topics and the ledger."""
    s = at(0, 14)
    lines = [
        (0.5, "화자 1", "결제 실패 처리부터 볼게요. 지난주에 실패가 좀 늘었어요.", False),
        (1.4, "화자 3", "대부분 카드사 쪽 일시 오류였어요. 잠깐 뒤에 다시 하면 거의 성공해요.", False),
        (2.1, "화자 1", "결제 실패하면 그냥 바로 다시 시도하면 안 돼요?", False),
        (2.6, "화자 2", "그거 전에도 한 번 얘기 나왔던 것 같은데요.", False),
        (2.9, "화자 2", "그때 왜 바로 재시도는 안 하기로 했죠?", True),
        (3.4, "화자 1", "아, 외부 API 한도 때문이었구나. 그럼 지금대로 가죠.", False),
        (4.2, "화자 3", "대신 실패 알림은 이메일만 가니까, 앱 푸시도 붙이면 좋겠어요.", False),
        (5.0, "화자 2", "푸시 문구는 제가 수요일까지 초안 써 올게요.", False),
        (5.6, "화자 1", "좋아요. 간격 그대로 두는 건 제가 문서에 적어 둘게요.", False),
    ]
    transcript = [{"key": f"e0:s1u{i + 1}", "at": iso(s + timedelta(minutes=mm)), "speaker": None, "text": text,
                   "question": q, "voice": voice} for i, (mm, voice, text, q) in enumerate(lines)]
    return meeting(s, 32, "remote", "결제 실패 재시도", [
        "결제 실패를 바로 다시 시도하자는 이야기가 나왔지만, 결제사 API 호출 한도 때문에 지금의 1분 · 5분 · 30분 간격을 "
        "그대로 두기로 했어요. 실패 알림에 앱 푸시를 더하기로 했어요."], transcript,
        topics=[
            {"title": "바로 재시도", "points": [
                "지난주 결제 실패는 대부분 카드사 쪽 일시 오류였어요.",
                "바로 다시 시도하면 결제사 API 호출 한도에 걸려서, 예전에 간격을 두기로 했던 걸 다시 확인했어요."]},
            {"title": "실패 알림", "points": ["지금은 이메일만 가요. 앱 푸시를 같이 보내기로 했어요."]}],
        voices=[{"key": "화자 1", "name": "민호"}, {"key": "화자 2", "name": "지안"}],
        ledger=[
            {"kind": "decision", "text": "재시도 간격은 1분 · 5분 · 30분 그대로 둬요.", "done": False},
            {"kind": "decision", "text": "결제 실패 알림에 앱 푸시를 더해요.", "done": False},
            {"kind": "commitment", "text": "푸시 알림 문구 초안 쓰기", "owner": "화자 2", "due": "수요일까지", "done": False},
            {"kind": "commitment", "text": "간격을 그대로 두는 이유를 결제 문서에 적기", "owner": "화자 1", "done": False}],
        questions=[{"id": "v1", "question": "그때 왜 바로 재시도는 안 하기로 했죠?", "asker": None, "voice": "화자 2",
                    "at": iso(s + timedelta(minutes=2.9)),
                    "answer": "결제사 API 호출 한도 때문이에요. 바로 다시 시도하면 한도에 걸려서 1분 · 5분 · 30분 간격을 두기로 했어요. [1]",
                    "cards": [card("c1", "docs/decisions/0012-결제-재시도-간격.md", "docs", "0012-결제-재시도-간격.md", 1),
                              card("c2", "src/payment/retry_policy.rs", "code", "retry_policy.rs", 2),
                              card("c3", "github/pull/412", "github", "PR #412 · 결제 재시도에 간격 두기", 3,
                                   "https://github.com/example/repo/pull/412")],
                    "cited": [1], "unanswered": False}])


def teams_meetings():
    """--teams: the landing's 할 일 / 결정 사항 pages (site 맨 위 장면 영상, 운영 회의 탭). Meetings of several teams over
    the last days, each leaving decisions and to-dos, so they gather in one place. People are named voices."""
    def m(days, h, minutes, mode, title, summary, voices, ledger):
        return meeting(at(days, h), minutes, mode, title, [summary], voices=[{"key": k, "name": v} for k, v in voices],
                       ledger=ledger)
    return [
        m(0, 10, 45, "inperson", "10월 운영 회의", "CS 외주 업체를 11월부터 바꾸고, 금요일을 재택 근무 날로 정했어요.",
          [("화자 1", "은지"), ("화자 2", "태호"), ("화자 3", "소연")], [
              {"kind": "decision", "text": "11월부터 CS 외주 업체를 바로응대로 바꿔요.", "done": False},
              {"kind": "decision", "text": "금요일은 재택 근무 날로 정해요.", "done": False},
              {"kind": "commitment", "text": "새 업체와 인수인계 일정 잡기", "owner": "화자 2", "due": "금요일까지", "done": False},
              {"kind": "commitment", "text": "재택 근무 규정 고쳐서 공지하기", "owner": "화자 3", "due": "다음 주 월요일까지", "done": False}]),
        m(0, 14, 30, "remote", "한빛물류 갱신 미팅", "갱신 할인은 계약서대로 10%이고, 늘리는 좌석 30개는 같은 조건으로 맞추기로 했어요.",
          [("화자 2", "준영")], [
              {"kind": "decision", "text": "한빛물류 갱신 할인은 계약서대로 10%예요.", "done": False},
              {"kind": "commitment", "text": "좌석 150개 기준 갱신 견적서 보내기", "owner": "화자 2", "due": "오늘까지", "done": False}]),
        m(1, 11, 40, "remote", "제품 주간 회의", "온보딩 체크리스트는 다시 넣지 않고, 첫 화면 예시 데이터로 가요.",
          [("화자 1", "서연"), ("화자 2", "민호"), ("화자 3", "지윤")], [
              {"kind": "decision", "text": "온보딩 체크리스트는 다시 넣지 않아요.", "done": False},
              {"kind": "commitment", "text": "첫 화면 예시 데이터 기획안 쓰기", "owner": "화자 1", "due": "목요일까지", "done": False},
              {"kind": "commitment", "text": "초대 버튼 위치 시안 공유하기", "owner": "화자 2", "done": False}]),
        m(2, 15, 35, "inperson", "채용 회의", "프로덕트 디자이너 최종 면접은 다음 주 화요일로 잡았어요.",
          [("화자 1", "소연")], [
              {"kind": "decision", "text": "디자이너 최종 면접은 다음 주 화요일에 해요.", "done": False},
              {"kind": "commitment", "text": "면접 일정 후보자에게 안내하기", "owner": "화자 1", "due": "화요일까지", "done": False}]),
        m(3, 16, 50, "remote", "마케팅 캠페인 리뷰", "연말 캠페인은 기존 고객 추천 이벤트로 가기로 했어요.",
          [("화자 1", "하은")], [
              {"kind": "decision", "text": "연말 캠페인은 기존 고객 추천 이벤트로 가요.", "done": False},
              {"kind": "commitment", "text": "추천 이벤트 랜딩 문구 초안 쓰기", "owner": "화자 1", "due": "다음 주", "done": False}]),
    ]


def main():
    out = Path(sys.argv[1])
    out.mkdir(parents=True, exist_ok=True)
    s = at(0, 14)
    lines = [
        (6, "them", "지금 결제 재시도는 몇 번까지 하게 돼 있어요?", True),
        (14, "me", "실패 알림은 어디서 보내요?", True),
        (19, "them", "재시도하다 이중 결제가 난 적이 있었나요?", True),
        (31, "them", "환불 API 타임아웃 값이 얼마로 잡혀 있죠?", True),
        (31.2, "me", "잠깐만요, 찾아볼게요.", False),
        (32, "them", "10초면 결제사 쪽 응답이 늦을 때 꽤 끊기겠네요.", False),
        (33, "me", "이건 원인을 좀 더 보고 다음 주에 다시 얘기해요.", False),
        (40, "me", "결제사마다 재시도 허용 횟수가 다른가요?", True),
        (41, "them", "그건 저도 확실하지 않아요. 문서로 정리해 두면 좋겠어요.", False),
        (44, "them", "그럼 3번 재시도하고, 마지막엔 이메일이랑 푸시로 가는 걸로 정리할게요.", False),
        (45, "me", "네, 간격 바꾸는 PR은 제가 금요일까지 올릴게요.", False),
        (46, "them", "알림 문구 초안은 제가 공유할게요.", False),
        (47, "me", "좋아요, 오늘은 여기까지 할게요.", False),
    ]
    transcript = [{"key": f"e0:s1u{i + 1}", "at": iso(s + timedelta(minutes=mm)), "speaker": who, "text": text, "question": q}
                  for i, (mm, who, text, q) in enumerate(lines)]
    q = lambda qid, mm, who, text, answer, cards, cited, missing=False: {  # noqa: E731
        "id": qid, "question": text, "asker": who, "at": iso(s + timedelta(minutes=mm)), "answer": answer, "cards": cards,
        "cited": cited, "unanswered": missing}
    first = meeting(s, 47, "remote", "결제 실패 재시도 정책", [
        "결제 실패는 3번까지 자동으로 다시 시도하고, 그래도 실패하면 고객에게 알리기로 했어요.",
        "다시 시도하는 간격은 지금의 고정 5분 대신 점점 늘리는 방식으로 바꿔요.",
        "환불 API 타임아웃은 원인을 더 보고 다음 주에 다시 이야기해요."], transcript,
        ledger=[
            {"kind": "decision", "text": "자동 재시도는 최대 3번까지 해요.", "done": False},
            {"kind": "decision", "text": "마지막 실패 뒤에는 이메일과 앱 푸시로 알려요.", "done": False},
            {"kind": "commitment", "text": "재시도 간격을 1분 · 5분 · 30분으로 바꾸는 PR 올리기", "owner": "me", "due": "금요일까지", "done": False},
            {"kind": "commitment", "text": "결제 실패 알림 문구 초안 공유하기", "owner": "them", "done": False},
            {"kind": "open_question", "text": "3번 모두 실패한 주문을 자동으로 취소할지", "done": False}],
        questions=[
            q("v1", 6, "them", "지금 결제 재시도는 몇 번까지 하게 돼 있어요?",
              "지금은 5분 간격으로 최대 2번 다시 시도해요. 횟수와 간격은 `PaymentRetryScheduler`에 상수로 들어 있어요. [1]",
              [card("c1", "apps/api/src/payment/payment-retry.scheduler.ts", "code", "payment-retry.scheduler.ts", 11),
               card("c2", "docs/결제/결제-실패-처리.md", "docs", "결제-실패-처리.md", 12),
               card("c3", "github/pull/1284", "github", "PR #1284 · 결제 재시도 큐 분리", 13, "https://github.com/example/repo/pull/1284")],
              [11]),
            q("v2", 14, "me", "실패 알림은 어디서 보내요?",
              "결제 실패 이벤트를 받으면 `NotificationService`가 이메일만 보내요. 앱 푸시는 아직 붙어 있지 않아요. [1]",
              [card("c4", "apps/api/src/notification/notification.service.ts", "code", "notification.service.ts", 21),
               card("c5", "apps/api/src/payment/payment.events.ts", "code", "payment.events.ts", 22)], [21]),
            q("v3", 19, "them", "재시도하다 이중 결제가 난 적이 있었나요?", None, [], [], True),
            q("v4", 31, "them", "환불 API 타임아웃 값이 얼마로 잡혀 있죠?", "환불 요청은 10초가 지나면 끊겨요. 결제사 클라이언트 설정에 있어요. [1]",
              [card("c6", "apps/api/src/payment/pg-client.config.ts", "code", "pg-client.config.ts", 31),
               card("c7", "github/issues/1301", "github", "이슈 #1301 · 환불 타임아웃", 32, "https://github.com/example/repo/issues/1301")],
              [31]),
            q("v5", 40, "me", "결제사마다 재시도 허용 횟수가 다른가요?", None, [], [], True)])
    meetings = [
        first,
        meeting(at(0, 11), 25, "inperson", "모바일 릴리스 점검",
                ["금요일 배포는 그대로 가요. 푸시 알림 권한 문구는 디자인팀이 다시 보내 주기로 했어요."]),
        meeting(at(1, 16), 38, "remote", "주간 백엔드 싱크",
                ["회원 탈퇴 처리 흐름을 정리했어요. 탈퇴 뒤 30일 보관 정책은 법무 확인을 기다려요."]),
        meeting(at(1, 10, 30), 52, "inperson", "신규 입사자 온보딩 Q&A",
                ["로컬 개발 환경과 배포 전 검사를 설명했어요. 문서에 답이 없던 질문 4개를 모았어요."]),
        meeting(at(2, 10), 61, "remote", "검색 지연 장애 회고",
                ["캐시 만료 시간이 짧아 DB 부하가 몰린 게 원인이었어요. 만료 시간과 알림 기준을 바꿔요."]),
        meeting(at(3, 15), 44, "inperson", "분기 계획 리뷰",
                ["4분기에는 결제 안정화와 검색 속도 개선을 먼저 해요. 어드민 개편은 1분기로 미뤘어요."]),
    ]
    if "--speakers" in sys.argv[2:]:
        meetings.insert(0, speakers_meeting())
    if "--teams" in sys.argv[2:]:
        meetings = teams_meetings()
    if "--landing" in sys.argv[2:]:
        meetings[0] = landing_meeting()  # takes the 14:00 meeting's place in the list
    for m in meetings:
        (out / f"{m['id']}.json").write_text(json.dumps(m, ensure_ascii=False, indent=2))
    print(f"wrote {len(meetings)} meetings to {out}")


if __name__ == "__main__":
    main()
