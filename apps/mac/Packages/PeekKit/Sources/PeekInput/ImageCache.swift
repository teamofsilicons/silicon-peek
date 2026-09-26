import CoreGraphics
import Foundation
import ImageIO
import OSLog
import PeekCore

/// Decodes show and option images for drawings and chrome (visual.md B7, BLUEPRINT §8.6):
///
/// * peekd has already copied every image into its content-addressed cache (`cache/images/<sha256>.<ext>`,
///   §1.7), so the UI only decodes: a `CGImage` of at most 512 px on the long side, orientation applied.
/// * `colors` = dominant colour + a 3–5 colour palette from k-means on a 32 × 32 downsample.
/// * Drawings get opaque handles (`{id, width, height}`) that belong to one send: ``release(sendID:)``
///   invalidates them, and ``image(for:)`` then returns nil (drawing an invalid handle does nothing).
/// * ``prepare(sendID:paths:)`` is idempotent per send: the drawing's InputHub and the chrome can both ask and get
///   the same handles. Decoded images are reused across sends (a DJ's repeated cover art) through a small
///   memory cache keyed by path, size and modification date.
@MainActor
public final class ImageCache: ImageProviding {
    public nonisolated static let maxPixelSize = 512
    /// Decoded images kept for reuse across sends.
    public static let reuseLimit = 48

    /// Relative image paths are resolved against ``PeekPaths/imageCacheDirectory``.
    public let paths: PeekPaths

    private struct Live {
        var image: CGImage
        var sendID: String
    }

    /// What one send has: its handles and the decodes still running for it. Replaced on release, so a decode
    /// that finishes after its send was released cannot bring handles back.
    private final class SendImages {
        var images: [String: PreparedImage] = [:]
        var inFlight: [String: Task<Decoded?, Never>] = [:]
    }

    private var live: [Int: Live] = [:]
    private var sends: [String: SendImages] = [:]
    private var reuse: [FileIdentity: Decoded] = [:]
    private var reuseOrder: [FileIdentity] = []
    private var nextID = 1
    private let logger = PeekLogger(category: "images")

    public init(paths: PeekPaths) {
        self.paths = paths
    }

    public func prepare(sendID: String, paths imagePaths: [String]) async -> [String: PreparedImage] {
        let send = sends[sendID] ?? SendImages()
        sends[sendID] = send
        var result: [String: PreparedImage] = [:]
        var seen = Set<String>()
        for path in imagePaths where seen.insert(path).inserted {
            if let image = send.images[path] {
                result[path] = image
                continue
            }
            let task = send.inFlight[path] ?? decodeTask(for: path)
            send.inFlight[path] = task
            let decoded = await task.value
            if send.inFlight[path] == task { send.inFlight[path] = nil }
            // Released while decoding: its handles must stay dead.
            guard sends[sendID] === send else { return [:] }
            if let image = send.images[path] {
                result[path] = image
                continue
            }
            guard let decoded else { continue }
            let id = nextID
            nextID += 1
            live[id] = Live(image: decoded.image, sendID: sendID)
            let image = PreparedImage(handle: ImageHandle(id: id, width: decoded.image.width, height: decoded.image.height),
                                      colors: decoded.colors)
            send.images[path] = image
            result[path] = image
        }
        return result
    }

    public func image(for handle: ImageHandle) -> CGImage? {
        guard let entry = live[handle.id], entry.image.width == handle.width, entry.image.height == handle.height else {
            return nil
        }
        return entry.image
    }

    public func release(sendID: String) {
        guard let send = sends.removeValue(forKey: sendID) else { return }
        for image in send.images.values { live.removeValue(forKey: image.handle.id) }
    }

    /// Live handles (for diagnostics and tests).
    public var liveHandleCount: Int { live.count }

    // MARK: Decoding

    struct Decoded: Sendable {
        var image: CGImage
        var colors: ImageColors
    }

    private func decodeTask(for path: String) -> Task<Decoded?, Never> {
        let url = resolve(path)
        let identity = FileIdentity(url: url)
        if let identity, let cached = reuse[identity] {
            touch(identity)
            return Task { cached }
        }
        let logger = self.logger
        return Task<Decoded?, Never> { [weak self] in
            let decoded = await Task.detached(priority: .userInitiated) { Self.decode(url) }.value
            guard let decoded else {
                logger.error("cannot decode image \(url.path); the element is drawn without it")
                return nil
            }
            if let identity { self?.remember(decoded, for: identity) }
            return decoded
        }
    }

    private func remember(_ decoded: Decoded, for identity: FileIdentity) {
        if reuse.updateValue(decoded, forKey: identity) == nil { reuseOrder.append(identity) } else { touch(identity) }
        while reuseOrder.count > Self.reuseLimit { reuse.removeValue(forKey: reuseOrder.removeFirst()) }
    }

    private func touch(_ identity: FileIdentity) {
        reuseOrder.removeAll { $0 == identity }
        reuseOrder.append(identity)
    }

    private func resolve(_ path: String) -> URL {
        let expanded = (path as NSString).expandingTildeInPath
        if expanded.hasPrefix("/") { return URL(fileURLWithPath: expanded) }
        return paths.imageCacheDirectory.appendingPathComponent(expanded)
    }

    /// Decodes `url` to ≤ 512 px with its orientation applied and computes its palette. Runs off the main thread.
    nonisolated static func decode(_ url: URL, maxPixelSize: Int = maxPixelSize) -> Decoded? {
        guard let source = CGImageSourceCreateWithURL(url as CFURL, [kCGImageSourceShouldCache: false] as CFDictionary),
            CGImageSourceGetCount(source) > 0
        else { return nil }
        let options: [CFString: Any] = [
            kCGImageSourceCreateThumbnailFromImageAlways: true,
            kCGImageSourceThumbnailMaxPixelSize: maxPixelSize,
            kCGImageSourceCreateThumbnailWithTransform: true,
            kCGImageSourceShouldCacheImmediately: true,
        ]
        let index = CGImageSourceGetPrimaryImageIndex(source)
        guard let image = CGImageSourceCreateThumbnailAtIndex(source, index, options as CFDictionary) else { return nil }
        let colors = PaletteExtractor.colors(of: image)
            ?? ImageColors(dominant: "#000000", palette: ["#000000", "#000000", "#000000"])
        return Decoded(image: image, colors: colors)
    }
}

/// Identifies a file's content cheaply: peekd's cache names files by their sha256, and size + modification date
/// guard against a file replaced in place.
struct FileIdentity: Hashable, Sendable {
    var path: String
    var size: Int
    var modified: Date

    init?(url: URL) {
        guard let attributes = try? FileManager.default.attributesOfItem(atPath: url.path),
            let size = (attributes[.size] as? NSNumber)?.intValue, let modified = attributes[.modificationDate] as? Date
        else { return nil }
        path = url.standardizedFileURL.path
        self.size = size
        self.modified = modified
    }
}
