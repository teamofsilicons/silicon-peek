import Foundation

/// The 44-byte canonical RIFF/WAVE header for linear PCM (notes/speech §4.3: "prepend a 44-byte RIFF/WAVE
/// header (PCM, 1 channel, 16000 Hz, 16-bit)"). peekd uploads the result through Peek to OpenAI as `audio/wav`.
public enum WAVFile {
    public static let headerSize = 44

    /// The header for `dataByteCount` bytes of interleaved little-endian PCM.
    public static func header(dataByteCount: Int, sampleRate: Int = 16_000, channels: Int = 1,
                              bitsPerSample: Int = 16) -> Data {
        precondition(dataByteCount >= 0 && dataByteCount <= Int(UInt32.max) - 36, "WAV data must fit in 4 GiB")
        let blockAlign = channels * bitsPerSample / 8
        let byteRate = sampleRate * blockAlign
        var header = Data(capacity: headerSize)
        header.append(contentsOf: Array("RIFF".utf8))
        header.appendLittleEndian(UInt32(36 + dataByteCount))
        header.append(contentsOf: Array("WAVE".utf8))
        header.append(contentsOf: Array("fmt ".utf8))
        header.appendLittleEndian(UInt32(16))  // size of the PCM fmt chunk
        header.appendLittleEndian(UInt16(1))  // WAVE_FORMAT_PCM
        header.appendLittleEndian(UInt16(channels))
        header.appendLittleEndian(UInt32(sampleRate))
        header.appendLittleEndian(UInt32(byteRate))
        header.appendLittleEndian(UInt16(blockAlign))
        header.appendLittleEndian(UInt16(bitsPerSample))
        header.append(contentsOf: Array("data".utf8))
        header.appendLittleEndian(UInt32(dataByteCount))
        return header
    }

    /// A complete WAV file: header + `pcm` (16-bit little-endian samples).
    public static func make(pcm: Data, sampleRate: Int = 16_000, channels: Int = 1) -> Data {
        var file = header(dataByteCount: pcm.count, sampleRate: sampleRate, channels: channels)
        file.append(pcm)
        return file
    }
}

extension Data {
    fileprivate mutating func appendLittleEndian<T: FixedWidthInteger>(_ value: T) {
        Swift.withUnsafeBytes(of: value.littleEndian) { append(contentsOf: $0) }
    }
}
