import Foundation

/// Words for a failure kind (engine errors.rs `ServiceError`, plus "engine" when the engine is not there), short enough
/// for the pill. `MeetingModel.say` puts the account's words first when Tacie AI is on.
enum FailureWords {
    static func say(_ kind: String) -> String {
        switch kind {
        case "network": return String(localized: "인터넷 연결이 끊겼어요")
        case "key": return String(localized: "서비스 키를 확인해 주세요")
        case "credit": return String(localized: "서비스 잔액이 부족해요")
        case "busy": return String(localized: "서비스가 붐벼서 기다리는 중이에요")
        case "engine": return String(localized: "검색 엔진에 연결하지 못했어요")
        case "model": return String(localized: "검색 모델을 받고 있어요")
        default: return String(localized: "외부 서비스가 응답하지 않아요")
        }
    }
}
