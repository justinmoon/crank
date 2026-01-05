// swift-tools-version: 5.9
import PackageDescription

let package = Package(
    name: "CrankAlertBadge",
    platforms: [.macOS(.v13)],
    targets: [
        .executableTarget(
            name: "CrankAlertBadge",
            path: "Sources"
        )
    ]
)
