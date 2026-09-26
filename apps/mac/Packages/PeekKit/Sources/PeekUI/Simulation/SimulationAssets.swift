import Foundation
import PeekCore

/// The files Simulation hands to the real pipeline: the cassette drawing (for `drawing.load`) and
/// the sample images (for `peek.show` image paths, which ``ImageProviding`` reads from disk).
///
/// They are generated on this Mac from code (``SimulationCassette``, ``SimulationArtwork``) into
/// `~/Library/Caches/Peek/Simulation/v<version>/`, so Simulation works offline and ships no
/// third-party media. Speech PCM is synthesized per text and streamed, never written to disk.
public struct SimulationAssetPaths: Sendable, Equatable {
    public var directory: URL
    public var cassette: URL
    public var cassetteSHA256: String

    public func cover(_ cover: SimulationArtwork.Cover) -> String {
        directory.appendingPathComponent(cover.rawValue + ".png").path
    }

    public func icon(_ icon: SimulationArtwork.Icon) -> String {
        directory.appendingPathComponent(icon.rawValue + ".png").path
    }

    /// Paths for validating a scenario before the files exist (``SimulationScenario/makeEvent(assets:sendID:askID:)``).
    public static let placeholder = SimulationAssetPaths(
        directory: URL(fileURLWithPath: "/nonexistent/peek-simulation"),
        cassette: URL(fileURLWithPath: "/nonexistent/peek-simulation/cassette.js"), cassetteSHA256: SimulationCassette.sha256)
}

public enum SimulationAssets {
    /// Bump when the artwork changes so old caches are not reused.
    public static let version = 1

    public struct Failure: Error, Equatable, Sendable, CustomStringConvertible {
        public let description: String
    }

    public static func directory(for paths: PeekPaths) -> URL {
        paths.cachesDirectory.appendingPathComponent("Simulation/v\(version)", isDirectory: true)
    }

    /// Writes the cassette and every image into `directory`. Images already present are kept;
    /// the cassette is rewritten whenever its bytes differ.
    public static func materialize(into directory: URL) throws(Failure) -> SimulationAssetPaths {
        let fileManager = FileManager.default
        let cassette = directory.appendingPathComponent(SimulationCassette.filename)
        try write(SimulationCassette.data, to: cassette, onlyIfDifferent: true)
        for cover in SimulationArtwork.Cover.allCases {
            let url = directory.appendingPathComponent(cover.rawValue + ".png")
            if isNonEmptyFile(url, fileManager) { continue }
            let png: Data
            do throws(SimulationArtwork.RenderError) {
                png = try SimulationArtwork.png(cover)
            } catch {
                throw Failure(description: "cannot draw the sample cover \(cover.rawValue): \(error.description)")
            }
            try write(png, to: url, onlyIfDifferent: false)
        }
        for icon in SimulationArtwork.Icon.allCases {
            let url = directory.appendingPathComponent(icon.rawValue + ".png")
            if isNonEmptyFile(url, fileManager) { continue }
            let png: Data
            do throws(SimulationArtwork.RenderError) {
                png = try SimulationArtwork.png(icon)
            } catch {
                throw Failure(description: "cannot draw the sample option image \(icon.rawValue): \(error.description)")
            }
            try write(png, to: url, onlyIfDifferent: false)
        }
        return SimulationAssetPaths(directory: directory, cassette: cassette, cassetteSHA256: SimulationCassette.sha256)
    }

    private static func isNonEmptyFile(_ url: URL, _ fileManager: FileManager) -> Bool {
        guard let attributes = try? fileManager.attributesOfItem(atPath: url.path),
            (attributes[.type] as? FileAttributeType) == .typeRegular,
            let size = attributes[.size] as? NSNumber
        else { return false }
        return size.intValue > 0
    }

    private static func write(_ data: Data, to url: URL, onlyIfDifferent: Bool) throws(Failure) {
        if onlyIfDifferent, let existing = try? Data(contentsOf: url), existing == data { return }
        do {
            try AtomicFile.write(data, to: url)
        } catch {
            throw Failure(
                description: "cannot write \(url.path): \(error). Simulation keeps its samples in ~/Library/Caches/Peek; "
                    + "check that the folder is writable.")
        }
    }
}
