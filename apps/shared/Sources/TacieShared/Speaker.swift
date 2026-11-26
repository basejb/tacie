import Foundation

/// Which channel an utterance came through, known only in remote meetings: the call's sound is them, the microphone is
/// you. The engine keeps the two channels' speakers apart; who spoke is its speaker key ("화자 2", Speaker 2), not this.
enum Speaker: String {
    case me, them
}
