import Foundation

/// A JSON document as parsed by ``StrictJSON``: the dynamic form of an IPC
/// frame header, unknown fields, `details` objects and answer values.
///
/// Integers without a fraction or exponent that fit in `Int64` are kept as
/// ``int(_:)`` so build numbers, frame counts and ids re-serialize exactly.
public enum JSONValue: Sendable, Hashable {
    case null
    case bool(Bool)
    case int(Int64)
    case double(Double)
    case string(String)
    case array([JSONValue])
    case object([String: JSONValue])

    public subscript(key: String) -> JSONValue? {
        if case .object(let fields) = self { return fields[key] }
        return nil
    }

    public var isNull: Bool {
        if case .null = self { return true }
        return false
    }

    public var stringValue: String? {
        if case .string(let value) = self { return value }
        return nil
    }

    public var boolValue: Bool? {
        if case .bool(let value) = self { return value }
        return nil
    }

    /// The value as an integer when it is an integer, or a double with no fractional part.
    public var intValue: Int? {
        switch self {
        case .int(let value): return Int(exactly: value)
        case .double(let value): return Int(exactly: value)
        default: return nil
        }
    }

    public var doubleValue: Double? {
        switch self {
        case .int(let value): return Double(value)
        case .double(let value): return value
        default: return nil
        }
    }

    public var arrayValue: [JSONValue]? {
        if case .array(let value) = self { return value }
        return nil
    }

    public var objectValue: [String: JSONValue]? {
        if case .object(let value) = self { return value }
        return nil
    }
}

extension JSONValue: ExpressibleByNilLiteral, ExpressibleByBooleanLiteral, ExpressibleByIntegerLiteral,
    ExpressibleByFloatLiteral, ExpressibleByStringLiteral, ExpressibleByArrayLiteral,
    ExpressibleByDictionaryLiteral
{
    public init(nilLiteral: ()) { self = .null }
    public init(booleanLiteral value: Bool) { self = .bool(value) }
    public init(integerLiteral value: Int64) { self = .int(value) }
    public init(floatLiteral value: Double) { self = .double(value) }
    public init(stringLiteral value: String) { self = .string(value) }
    public init(arrayLiteral elements: JSONValue...) { self = .array(elements) }
    public init(dictionaryLiteral elements: (String, JSONValue)...) {
        self = .object(Dictionary(elements, uniquingKeysWith: { _, last in last }))
    }
}

extension JSONValue: Codable {
    public init(from decoder: any Decoder) throws {
        let container = try decoder.singleValueContainer()
        if container.decodeNil() {
            self = .null
        } else if let value = try? container.decode(Bool.self) {
            self = .bool(value)
        } else if let value = try? container.decode(Int64.self) {
            self = .int(value)
        } else if let value = try? container.decode(Double.self) {
            self = .double(value)
        } else if let value = try? container.decode(String.self) {
            self = .string(value)
        } else if let value = try? container.decode([JSONValue].self) {
            self = .array(value)
        } else if let value = try? container.decode([String: JSONValue].self) {
            self = .object(value)
        } else {
            throw DecodingError.dataCorruptedError(
                in: container, debugDescription: "value is not representable as JSON")
        }
    }

    public func encode(to encoder: any Encoder) throws {
        var container = encoder.singleValueContainer()
        switch self {
        case .null: try container.encodeNil()
        case .bool(let value): try container.encode(value)
        case .int(let value): try container.encode(value)
        case .double(let value): try container.encode(value)
        case .string(let value): try container.encode(value)
        case .array(let value): try container.encode(value)
        case .object(let value): try container.encode(value)
        }
    }
}

// MARK: - Serialization

extension JSONValue {
    /// Compact UTF-8 JSON with object keys sorted, so equal values serialize to equal bytes.
    /// Non-finite doubles (which JSON cannot represent) are written as `null`.
    public func serialized() -> Data {
        var out: [UInt8] = []
        out.reserveCapacity(256)
        write(into: &out)
        return Data(out)
    }

    /// The serialized form as a `String`.
    public var jsonString: String { String(decoding: serialized(), as: UTF8.self) }

    public func write(into out: inout [UInt8]) {
        switch self {
        case .null:
            JSONWriter.writeNull(into: &out)
        case .bool(let value):
            JSONWriter.writeBool(value, into: &out)
        case .int(let value):
            out.append(contentsOf: Array(String(value).utf8))
        case .double(let value):
            JSONWriter.writeNumber(value, into: &out)
        case .string(let value):
            JSONWriter.writeString(value, into: &out)
        case .array(let values):
            out.append(UInt8(ascii: "["))
            for (index, value) in values.enumerated() {
                if index > 0 { out.append(UInt8(ascii: ",")) }
                value.write(into: &out)
            }
            out.append(UInt8(ascii: "]"))
        case .object(let fields):
            out.append(UInt8(ascii: "{"))
            for (index, key) in fields.keys.sorted().enumerated() {
                if index > 0 { out.append(UInt8(ascii: ",")) }
                JSONWriter.writeString(key, into: &out)
                out.append(UInt8(ascii: ":"))
                fields[key, default: .null].write(into: &out)
            }
            out.append(UInt8(ascii: "}"))
        }
    }
}

/// Low-level JSON output helpers shared by ``JSONValue`` and the per-frame
/// ``InputSnapshot`` encoder.
public enum JSONWriter {
    private static let hex: [UInt8] = Array("0123456789abcdef".utf8)
    private static let nullBytes: [UInt8] = Array("null".utf8)
    private static let trueBytes: [UInt8] = Array("true".utf8)
    private static let falseBytes: [UInt8] = Array("false".utf8)

    /// Writes a JSON string literal, escaping `"`, `\` and control characters.
    public static func writeString(_ value: String, into out: inout [UInt8]) {
        out.append(UInt8(ascii: "\""))
        for byte in value.utf8 {
            switch byte {
            case UInt8(ascii: "\""): out.append(contentsOf: [UInt8(ascii: "\\"), UInt8(ascii: "\"")])
            case UInt8(ascii: "\\"): out.append(contentsOf: [UInt8(ascii: "\\"), UInt8(ascii: "\\")])
            case 0x0A: out.append(contentsOf: [UInt8(ascii: "\\"), UInt8(ascii: "n")])
            case 0x0D: out.append(contentsOf: [UInt8(ascii: "\\"), UInt8(ascii: "r")])
            case 0x09: out.append(contentsOf: [UInt8(ascii: "\\"), UInt8(ascii: "t")])
            case 0x08: out.append(contentsOf: [UInt8(ascii: "\\"), UInt8(ascii: "b")])
            case 0x0C: out.append(contentsOf: [UInt8(ascii: "\\"), UInt8(ascii: "f")])
            case 0x00..<0x20:
                out.append(contentsOf: [UInt8(ascii: "\\"), UInt8(ascii: "u"), UInt8(ascii: "0"), UInt8(ascii: "0")])
                out.append(hex[Int(byte >> 4)])
                out.append(hex[Int(byte & 0x0F)])
            default:
                out.append(byte)
            }
        }
        out.append(UInt8(ascii: "\""))
    }

    /// Writes a finite double in its shortest round-tripping form; NaN and ±∞ become `null`.
    public static func writeNumber(_ value: Double, into out: inout [UInt8]) {
        guard value.isFinite else {
            out.append(contentsOf: nullBytes)
            return
        }
        if value == value.rounded(.towardZero), abs(value) < 1e15 {
            out.append(contentsOf: Array(String(Int64(value)).utf8))
        } else {
            out.append(contentsOf: Array(value.description.utf8))
        }
    }

    public static func writeBool(_ value: Bool, into out: inout [UInt8]) {
        out.append(contentsOf: value ? trueBytes : falseBytes)
    }

    public static func writeNull(into out: inout [UInt8]) {
        out.append(contentsOf: nullBytes)
    }
}

// MARK: - Strict parsing

/// An RFC 8259 parser that is stricter than `JSONSerialization`:
/// duplicate object keys, invalid UTF-8, lone surrogates, unescaped control
/// characters, leading zeros, trailing garbage and nesting deeper than
/// ``maxDepth`` are all errors. This mirrors `ting_client::strict_json`
/// (BLUEPRINT §1.6: "duplicate keys are rejected").
public enum StrictJSON {
    public static let maxDepth = 128

    public struct ParseError: Error, Equatable, CustomStringConvertible, Sendable {
        /// Byte offset in the input where the problem was found.
        public let offset: Int
        public let reason: String

        public var description: String { "invalid JSON at byte \(offset): \(reason)" }
    }

    public static func parse(_ data: Data) throws(ParseError) -> JSONValue {
        let bytes = [UInt8](data)
        return try parse(bytes: bytes)
    }

    public static func parse(_ string: String) throws(ParseError) -> JSONValue {
        try parse(bytes: Array(string.utf8))
    }

    public static func parse(bytes: [UInt8]) throws(ParseError) -> JSONValue {
        var parser = Parser(bytes: bytes)
        parser.skipWhitespace()
        let value = try parser.parseValue(depth: 0)
        parser.skipWhitespace()
        guard parser.index == bytes.count else {
            throw ParseError(offset: parser.index, reason: "unexpected data after the top-level value")
        }
        return value
    }

    private struct Parser {
        let bytes: [UInt8]
        var index = 0

        init(bytes: [UInt8]) { self.bytes = bytes }

        mutating func skipWhitespace() {
            while index < bytes.count {
                switch bytes[index] {
                case 0x20, 0x09, 0x0A, 0x0D: index += 1
                default: return
                }
            }
        }

        func fail(_ reason: String, at offset: Int? = nil) -> ParseError {
            ParseError(offset: offset ?? index, reason: reason)
        }

        mutating func parseValue(depth: Int) throws(ParseError) -> JSONValue {
            guard depth <= StrictJSON.maxDepth else {
                throw fail("nesting deeper than \(StrictJSON.maxDepth) levels")
            }
            guard index < bytes.count else { throw fail("unexpected end of input, expected a value") }
            switch bytes[index] {
            case UInt8(ascii: "{"): return try parseObject(depth: depth)
            case UInt8(ascii: "["): return try parseArray(depth: depth)
            case UInt8(ascii: "\""): return .string(try parseString())
            case UInt8(ascii: "t"): try expectLiteral("true"); return .bool(true)
            case UInt8(ascii: "f"): try expectLiteral("false"); return .bool(false)
            case UInt8(ascii: "n"): try expectLiteral("null"); return .null
            case UInt8(ascii: "-"), UInt8(ascii: "0")...UInt8(ascii: "9"): return try parseNumber()
            default:
                throw fail("unexpected character \(Self.describe(bytes[index])), expected a value")
            }
        }

        mutating func expectLiteral(_ literal: StaticString) throws(ParseError) {
            let start = index
            let text = literal.withUTF8Buffer { Array($0) }
            guard index + text.count <= bytes.count, Array(bytes[index..<index + text.count]) == text else {
                throw fail("invalid literal, expected \(literal)", at: start)
            }
            index += text.count
        }

        mutating func parseObject(depth: Int) throws(ParseError) -> JSONValue {
            index += 1  // {
            var fields: [String: JSONValue] = [:]
            skipWhitespace()
            if index < bytes.count, bytes[index] == UInt8(ascii: "}") {
                index += 1
                return .object(fields)
            }
            while true {
                skipWhitespace()
                guard index < bytes.count, bytes[index] == UInt8(ascii: "\"") else {
                    throw fail("expected a string key in object")
                }
                let keyOffset = index
                let key = try parseString()
                skipWhitespace()
                guard index < bytes.count, bytes[index] == UInt8(ascii: ":") else {
                    throw fail("expected ':' after object key \"\(key)\"")
                }
                index += 1
                skipWhitespace()
                let value = try parseValue(depth: depth + 1)
                if fields.updateValue(value, forKey: key) != nil {
                    throw fail("duplicate key \"\(key)\"", at: keyOffset)
                }
                skipWhitespace()
                guard index < bytes.count else { throw fail("unexpected end of input inside an object") }
                if bytes[index] == UInt8(ascii: ",") {
                    index += 1
                    continue
                }
                if bytes[index] == UInt8(ascii: "}") {
                    index += 1
                    return .object(fields)
                }
                throw fail("expected ',' or '}' in object, found \(Self.describe(bytes[index]))")
            }
        }

        mutating func parseArray(depth: Int) throws(ParseError) -> JSONValue {
            index += 1  // [
            var values: [JSONValue] = []
            skipWhitespace()
            if index < bytes.count, bytes[index] == UInt8(ascii: "]") {
                index += 1
                return .array(values)
            }
            while true {
                skipWhitespace()
                values.append(try parseValue(depth: depth + 1))
                skipWhitespace()
                guard index < bytes.count else { throw fail("unexpected end of input inside an array") }
                if bytes[index] == UInt8(ascii: ",") {
                    index += 1
                    continue
                }
                if bytes[index] == UInt8(ascii: "]") {
                    index += 1
                    return .array(values)
                }
                throw fail("expected ',' or ']' in array, found \(Self.describe(bytes[index]))")
            }
        }

        mutating func parseString() throws(ParseError) -> String {
            let start = index
            index += 1  // opening quote
            var buffer: [UInt8] = []
            while true {
                guard index < bytes.count else { throw fail("unterminated string", at: start) }
                let byte = bytes[index]
                switch byte {
                case UInt8(ascii: "\""):
                    index += 1
                    guard let string = String(validating: buffer, as: UTF8.self) else {
                        throw fail("string is not valid UTF-8", at: start)
                    }
                    return string
                case UInt8(ascii: "\\"):
                    index += 1
                    guard index < bytes.count else { throw fail("unterminated escape sequence") }
                    let escape = bytes[index]
                    index += 1
                    switch escape {
                    case UInt8(ascii: "\""): buffer.append(0x22)
                    case UInt8(ascii: "\\"): buffer.append(0x5C)
                    case UInt8(ascii: "/"): buffer.append(0x2F)
                    case UInt8(ascii: "b"): buffer.append(0x08)
                    case UInt8(ascii: "f"): buffer.append(0x0C)
                    case UInt8(ascii: "n"): buffer.append(0x0A)
                    case UInt8(ascii: "r"): buffer.append(0x0D)
                    case UInt8(ascii: "t"): buffer.append(0x09)
                    case UInt8(ascii: "u"):
                        let scalar = try parseUnicodeEscape()
                        buffer.append(contentsOf: Array(String(Character(scalar)).utf8))
                    default:
                        throw fail("invalid escape \\\(Self.describe(escape))", at: index - 2)
                    }
                case 0x00..<0x20:
                    throw fail("unescaped control character \(Self.describe(byte)) in string")
                default:
                    buffer.append(byte)
                    index += 1
                }
            }
        }

        mutating func parseHex4() throws(ParseError) -> UInt32 {
            guard index + 4 <= bytes.count else { throw fail("truncated \\u escape") }
            var value: UInt32 = 0
            for _ in 0..<4 {
                let byte = bytes[index]
                let digit: UInt32
                switch byte {
                case UInt8(ascii: "0")...UInt8(ascii: "9"): digit = UInt32(byte - UInt8(ascii: "0"))
                case UInt8(ascii: "a")...UInt8(ascii: "f"): digit = UInt32(byte - UInt8(ascii: "a") + 10)
                case UInt8(ascii: "A")...UInt8(ascii: "F"): digit = UInt32(byte - UInt8(ascii: "A") + 10)
                default: throw fail("invalid hex digit \(Self.describe(byte)) in \\u escape")
                }
                value = value << 4 | digit
                index += 1
            }
            return value
        }

        mutating func parseUnicodeEscape() throws(ParseError) -> Unicode.Scalar {
            let start = index - 2
            let first = try parseHex4()
            if (0xD800...0xDBFF).contains(first) {
                guard index + 2 <= bytes.count, bytes[index] == UInt8(ascii: "\\"),
                    bytes[index + 1] == UInt8(ascii: "u")
                else {
                    throw fail("lone high surrogate \\u\(String(first, radix: 16))", at: start)
                }
                index += 2
                let second = try parseHex4()
                guard (0xDC00...0xDFFF).contains(second) else {
                    throw fail("high surrogate not followed by a low surrogate", at: start)
                }
                let combined = 0x10000 + ((first - 0xD800) << 10) + (second - 0xDC00)
                guard let scalar = Unicode.Scalar(combined) else {
                    throw fail("invalid surrogate pair", at: start)
                }
                return scalar
            }
            if (0xDC00...0xDFFF).contains(first) {
                throw fail("lone low surrogate \\u\(String(first, radix: 16))", at: start)
            }
            guard let scalar = Unicode.Scalar(first) else { throw fail("invalid \\u escape", at: start) }
            return scalar
        }

        mutating func parseNumber() throws(ParseError) -> JSONValue {
            let start = index
            var isInteger = true
            if bytes[index] == UInt8(ascii: "-") { index += 1 }
            guard index < bytes.count, Self.isDigit(bytes[index]) else {
                throw fail("expected a digit in number", at: start)
            }
            if bytes[index] == UInt8(ascii: "0") {
                index += 1
                if index < bytes.count, Self.isDigit(bytes[index]) {
                    throw fail("numbers must not have leading zeros", at: start)
                }
            } else {
                while index < bytes.count, Self.isDigit(bytes[index]) { index += 1 }
            }
            if index < bytes.count, bytes[index] == UInt8(ascii: ".") {
                isInteger = false
                index += 1
                guard index < bytes.count, Self.isDigit(bytes[index]) else {
                    throw fail("expected a digit after the decimal point", at: start)
                }
                while index < bytes.count, Self.isDigit(bytes[index]) { index += 1 }
            }
            if index < bytes.count, bytes[index] == UInt8(ascii: "e") || bytes[index] == UInt8(ascii: "E") {
                isInteger = false
                index += 1
                if index < bytes.count, bytes[index] == UInt8(ascii: "+") || bytes[index] == UInt8(ascii: "-") {
                    index += 1
                }
                guard index < bytes.count, Self.isDigit(bytes[index]) else {
                    throw fail("expected a digit in the exponent", at: start)
                }
                while index < bytes.count, Self.isDigit(bytes[index]) { index += 1 }
            }
            let text = String(decoding: bytes[start..<index], as: UTF8.self)
            if isInteger, let value = Int64(text) {
                return .int(value)
            }
            guard let value = Double(text), value.isFinite else {
                throw fail("number \(text) is out of range", at: start)
            }
            return .double(value)
        }

        static func isDigit(_ byte: UInt8) -> Bool { byte >= UInt8(ascii: "0") && byte <= UInt8(ascii: "9") }

        static func describe(_ byte: UInt8) -> String {
            if byte >= 0x21, byte < 0x7F { return "'\(Character(Unicode.Scalar(byte)))'" }
            return "0x" + String(byte, radix: 16, uppercase: true)
        }
    }
}
