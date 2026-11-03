// swift-tools-version: 6.0

import PackageDescription

let package = Package(
    name: "Otag",
    platforms: [.macOS(.v14)],
    products: [
        .executable(name: "Otag", targets: ["Otag"]),
        .library(name: "OtagKit", targets: ["OtagKit"]),
    ],
    dependencies: [
        // The terminal emulator and view. MIT, and the only dependency the
        // app has: everything else is the system's own frameworks.
        .package(url: "https://github.com/migueldeicaza/SwiftTerm", from: "1.2.0"),
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
            dependencies: ["OtagKit", .product(name: "SwiftTerm", package: "SwiftTerm")],
            swiftSettings: [.swiftLanguageMode(.v5)]
        ),
        .testTarget(
            name: "OtagKitTests",
            dependencies: ["OtagKit"],
            swiftSettings: [.swiftLanguageMode(.v5)]
        ),
    ]
)
