// swift-tools-version: 6.2
// 6.2 is required for `.macOS(.v26)`; 6.0 fails with "'v26' is unavailable" (BLUEPRINT §8.1).
//
// quickjs-ng v0.17.0, vendored unmodified by scripts/vendor-quickjs.sh (sha256 pinned there),
// plus peek_qjs.c / include/peek_qjs.h: the only API Swift calls. quickjs.h is deliberately not in
// include/, so Swift cannot reach the raw QuickJS API or its compound-literal JSValue macros.
import PackageDescription

let package = Package(
    name: "CQuickJS",
    platforms: [.macOS(.v26)],
    products: [
        .library(name: "CQuickJS", targets: ["CQuickJS"])
    ],
    targets: [
        .target(
            name: "CQuickJS",
            exclude: [],
            sources: ["quickjs.c", "libregexp.c", "libunicode.c", "dtoa.c", "peek_qjs.c"],
            publicHeadersPath: "include",
            cSettings: [
                .define("_GNU_SOURCE"),
                .define("QUICKJS_NG_BUILD"),
                .unsafeFlags([
                    // Optimised even in Debug: at -O0 QuickJS is 5–10× slower and a drawing's 4 ms frame budget is
                    // hard to meet (drawing agent's request).
                    "-O2",
                    "-funsigned-char",
                    "-Wno-implicit-fallthrough",
                    "-Wno-sign-compare",
                    "-Wno-missing-field-initializers",
                    "-Wno-unused-parameter",
                    "-Wno-unused-but-set-variable",
                    // Xcode turns this on for package targets; upstream quickjs-ng is not clean under it.
                    "-Wno-shorten-64-to-32",
                ]),
            ]
        ),
    ],
    cLanguageStandard: .gnu11
)
