import CoreGraphics
import Foundation
import PeekCore

/// `colors.dominant` and `colors.palette` of a show or option image (visual.md B7): k-means on a 32 × 32
/// downsample. Deterministic (seeded k-means++), so the same image always gives the same palette.
///
/// * Up to 5 clusters; clusters closer than ``mergeDistance`` merge, and clusters holding less than
///   ``minimumShare`` of the pixels fold into their nearest neighbour.
/// * The palette is ordered by population; `dominant` is the most populous cluster.
/// * Drawings may index `palette[0…2]`, so a flat image with fewer distinct colours repeats its colours to
///   reach 3 entries. Nothing is invented.
public enum PaletteExtractor {
    public static let sampleSize = 32
    public static let maxColors = 5
    public static let minColors = 3
    /// Euclidean distance in 0…1 sRGB below which two cluster centres count as one colour.
    public static let mergeDistance = 0.08
    public static let minimumShare = 0.03
    /// Pixels more transparent than this are ignored.
    public static let minimumAlpha = 0.5
    static let iterations = 16

    /// The palette of `image` (downsampled to 32 × 32 first). Nil when the image has no opaque pixels.
    public static func colors(of image: CGImage) -> ImageColors? {
        guard let bitmap = RGBABitmap(image: image, width: sampleSize, height: sampleSize) else { return nil }
        return colors(of: bitmap)
    }

    /// The palette of an already-downsampled bitmap.
    public static func colors(of bitmap: RGBABitmap) -> ImageColors? {
        var samples: [SIMD3<Double>] = []
        samples.reserveCapacity(bitmap.width * bitmap.height)
        for y in 0..<bitmap.height {
            for x in 0..<bitmap.width {
                let (color, alpha) = bitmap.pixel(x: x, y: y)
                guard alpha >= minimumAlpha else { continue }
                samples.append(SIMD3(color.red, color.green, color.blue))
            }
        }
        guard !samples.isEmpty else { return nil }
        let clusters = cluster(samples)
        var palette = clusters.map { SRGBColor(red: $0.center.x, green: $0.center.y, blue: $0.center.z).hex }
        let distinct = palette
        var index = 0
        while palette.count < minColors {
            palette.append(distinct[index % distinct.count])
            index += 1
        }
        return ImageColors(dominant: palette[0], palette: palette)
    }

    struct Cluster: Equatable {
        var center: SIMD3<Double>
        var count: Int
    }

    /// k-means (k = min(5, distinct samples)), then merging and folding; ordered by population, largest first.
    static func cluster(_ samples: [SIMD3<Double>]) -> [Cluster] {
        let distinctCount = Set(samples.map { SIMD3<Int>(Int($0.x * 255), Int($0.y * 255), Int($0.z * 255)) }).count
        let k = max(1, min(maxColors, distinctCount))
        var centers = seed(samples, k: k)
        var assignment = [Int](repeating: 0, count: samples.count)
        for _ in 0..<iterations {
            var changed = false
            for (i, sample) in samples.enumerated() {
                let nearest = nearestIndex(of: sample, in: centers)
                if nearest != assignment[i] {
                    assignment[i] = nearest
                    changed = true
                }
            }
            var sums = [SIMD3<Double>](repeating: .zero, count: centers.count)
            var counts = [Int](repeating: 0, count: centers.count)
            for (i, sample) in samples.enumerated() {
                sums[assignment[i]] += sample
                counts[assignment[i]] += 1
            }
            for c in centers.indices where counts[c] > 0 { centers[c] = sums[c] / Double(counts[c]) }
            if !changed { break }
        }
        var clusters = centers.indices.map { c in
            Cluster(center: centers[c], count: assignment.lazy.filter { $0 == c }.count)
        }.filter { $0.count > 0 }
        clusters = merge(clusters, total: samples.count)
        return clusters.sorted { lhs, rhs in
            lhs.count != rhs.count ? lhs.count > rhs.count : lexicographicallyPrecedes(lhs.center, rhs.center)
        }
    }

    /// Merges near-identical centres and folds tiny clusters into their nearest neighbour.
    static func merge(_ input: [Cluster], total: Int) -> [Cluster] {
        var clusters = input
        var merged = true
        while merged, clusters.count > 1 {
            merged = false
            var best: (Int, Int, Double)?
            for i in clusters.indices {
                for j in clusters.indices where j > i {
                    let distance = length(clusters[i].center - clusters[j].center)
                    if distance < mergeDistance, distance < (best?.2 ?? .infinity) { best = (i, j, distance) }
                }
            }
            if let (i, j, _) = best {
                clusters[i] = combine(clusters[i], clusters[j])
                clusters.remove(at: j)
                merged = true
            }
        }
        let minimum = Int((Double(total) * minimumShare).rounded(.up))
        while clusters.count > 1, let smallest = clusters.indices.min(by: { clusters[$0].count < clusters[$1].count }),
            clusters[smallest].count < minimum
        {
            let small = clusters.remove(at: smallest)
            let target = nearestIndex(of: small.center, in: clusters.map(\.center))
            clusters[target] = combine(clusters[target], small)
        }
        return clusters
    }

    private static func combine(_ a: Cluster, _ b: Cluster) -> Cluster {
        let count = a.count + b.count
        return Cluster(center: (a.center * Double(a.count) + b.center * Double(b.count)) / Double(count), count: count)
    }

    /// k-means++ seeding with a fixed-seed generator.
    static func seed(_ samples: [SIMD3<Double>], k: Int) -> [SIMD3<Double>] {
        var generator = SplitMix64(seed: 0x5EED_9EEC)
        var centers = [samples[Int(generator.next() % UInt64(samples.count))]]
        var distances = samples.map { lengthSquared($0 - centers[0]) }
        while centers.count < k {
            let total = distances.reduce(0, +)
            guard total > 0 else { break }
            var target = Double(generator.next() >> 11) / Double(1 << 53) * total
            var chosen = samples.count - 1
            for (i, d) in distances.enumerated() {
                target -= d
                if target <= 0 {
                    chosen = i
                    break
                }
            }
            centers.append(samples[chosen])
            for i in samples.indices { distances[i] = min(distances[i], lengthSquared(samples[i] - samples[chosen])) }
        }
        return centers
    }

    static func nearestIndex(of sample: SIMD3<Double>, in centers: [SIMD3<Double>]) -> Int {
        var best = 0
        var bestDistance = Double.infinity
        for (index, center) in centers.enumerated() {
            let distance = lengthSquared(sample - center)
            if distance < bestDistance {
                best = index
                bestDistance = distance
            }
        }
        return best
    }

    private static func lengthSquared(_ v: SIMD3<Double>) -> Double { (v * v).sum() }
    private static func length(_ v: SIMD3<Double>) -> Double { lengthSquared(v).squareRoot() }
    private static func lexicographicallyPrecedes(_ a: SIMD3<Double>, _ b: SIMD3<Double>) -> Bool {
        (a.x, a.y, a.z) < (b.x, b.y, b.z)
    }
}

/// A tiny deterministic PRNG (Steele, Lea & Flood's SplitMix64) for reproducible k-means seeding.
struct SplitMix64: RandomNumberGenerator {
    private var state: UInt64

    init(seed: UInt64) { state = seed }

    mutating func next() -> UInt64 {
        state &+= 0x9E37_79B9_7F4A_7C15
        var z = state
        z = (z ^ (z >> 30)) &* 0xBF58_476D_1CE4_E5B9
        z = (z ^ (z >> 27)) &* 0x94D0_49BB_1331_11EB
        return z ^ (z >> 31)
    }
}
