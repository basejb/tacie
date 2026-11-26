// swift-tools-version:5.9
import PackageDescription

let package = Package(
    name: "TacieShared",
    platforms: [.macOS(.v14), .iOS(.v17)],
    targets: [.target(name: "TacieShared", path: "Sources/TacieShared")]
)
