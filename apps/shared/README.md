# apps/shared

Swift code shared by the Mac app and the official edition's iPhone app. The iPhone app is not in the public repository.
This is a **shared source folder**, not a Swift package (module): each app target compiles `Sources/TacieShared` as its
own sources (`apps/mac/project.yml`). So nothing here is marked `public`, and `String(localized:)` strings end up in each
app's `Localizable.xcstrings`.

What is in it: meeting records (`SavedMeeting`, `MeetingText`, `MeetingDigest`, `MeetingVoices`, `NotesKind`, `DueDate`,
`Speaker`, `ShadowResult`, attendees `Person` and `MeetingPrint`), the engine client (`EngineClient` and its response
types), and small helpers (`Log`, `Build`, `AppLocale`, `MeetingTranslation`).

What only the Mac has (connected sources, the storage location `AccountSpace`, the recorder, the attendee store
`VoiceVault`) stays in `apps/mac`. Mac-only functions on shared types live in separate extensions
(`EngineClient+Connections.swift`, `MeetingDigest+Library.swift`).

## Keeping the boundary

`Package.swift` exists only to check that this folder builds on its own: a Mac-only type in a shared file breaks it.
Run both after moving or changing files.

```sh
cd apps/shared
swift build
xcodebuild -scheme TacieShared-Package -destination 'generic/platform=iOS' -derivedDataPath .build/xc build
```
