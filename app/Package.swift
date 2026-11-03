// swift-tools-version: 6.0

import PackageDescription

let package = Package(
    name: "Otag",
    platforms: [.macOS(.v14)],
    products: [
        .executable(name: "Otag", targets: ["Otag"]),
        .library(name: "OtagKit", targets: ["OtagKit"]),
    ],
    targets: [
        // Everything that talks to the daemon, with no user interface in it,
        // so that it can be tested on its own.
        .target(
            name: "OtagKit",
            swiftSettings: [.swiftLanguageMode(.v5)]
        ),
        // The app itself: views and nothing else worth testing.
        .executableTarget(
            name: "Otag",
            dependencies: ["OtagKit"],
            swiftSettings: [.swiftLanguageMode(.v5)]
        ),
        .testTarget(
            name: "OtagKitTests",
            dependencies: ["OtagKit"],
            swiftSettings: [.swiftLanguageMode(.v5)]
        ),
    ]
)
