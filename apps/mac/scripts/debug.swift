// Sends a debug command to a Tacie launched with --debug-control.
// usage: swift scripts/debug.swift pause | stop | ending | "start remote" | "replay inperson /path.wav"
//        | "ask <question>" | history
// (the full list is in AppDelegate.listenForDebugCommands)
// TACIE_DEBUG_CHANNEL=<name> reaches only the app launched with --debug-channel <name>.
import Foundation
let cmd = CommandLine.arguments.dropFirst().joined(separator: " ")
let channel = ProcessInfo.processInfo.environment["TACIE_DEBUG_CHANNEL"].map { ".\($0)" } ?? ""
DistributedNotificationCenter.default().postNotificationName(Notification.Name("ai.tacie.debug" + channel), object: cmd, userInfo: nil, deliverImmediately: true)
