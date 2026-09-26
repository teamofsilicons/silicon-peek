import Foundation
import Testing

@testable import PeekCore

@Suite("StrictJSON")
struct StrictJSONTests {
    @Test("parses every JSON type and keeps integers exact")
    func parsesValues() throws {
        let value = try StrictJSON.parse(#" {"a":[1,-2,3.5,1e3,true,false,null],"b":{"c":"d"},"big":9007199254740993} "#)
        #expect(value["a"] == .array([.int(1), .int(-2), .double(3.5), .double(1000), .bool(true), .bool(false), .null]))
        #expect(value["b"]?["c"] == .string("d"))
        #expect(value["big"] == .int(9_007_199_254_740_993))
    }

    @Test("decodes escapes, including surrogate pairs")
    func escapes() throws {
        let value = try StrictJSON.parse(#""a\"b\\c\/d\n\té😀""#)
        #expect(value == .string("a\"b\\c/d\n\té😀"))
    }

    @Test(
        "rejects invalid documents with a precise reason",
        arguments: [
            (#"{"a":1,"a":2}"#, "duplicate key \"a\""),
            (#"{"x":{"y":1,"y":1}}"#, "duplicate key \"y\""),
            (#"{"a":1} x"#, "unexpected data after the top-level value"),
            (#"{"a":01}"#, "leading zeros"),
            (#""\ud800""#, "lone high surrogate"),
            (#""\udc00""#, "lone low surrogate"),
            ("\"a\u{01}b\"", "unescaped control character"),
            (#"{"a":1,}"#, "expected a string key"),
            (#"[1,]"#, "expected a value"),
            (#"{"a" 1}"#, "expected ':'"),
            (#"tru"#, "invalid literal"),
            (#"1e999"#, "out of range"),
            (#""\x""#, "invalid escape"),
            ("", "unexpected end of input"),
        ])
    func rejects(input: String, reason: String) {
        #expect {
            try StrictJSON.parse(input)
        } throws: { error in
            guard let error = error as? StrictJSON.ParseError else { return false }
            return error.reason.contains(reason)
        }
    }

    @Test("rejects invalid UTF-8 inside strings")
    func invalidUTF8() {
        let bytes: [UInt8] = [0x22, 0xC3, 0x28, 0x22]
        #expect(throws: StrictJSON.ParseError.self) { try StrictJSON.parse(bytes: bytes) }
    }

    @Test("rejects nesting deeper than the limit")
    func depthLimit() {
        let deep = String(repeating: "[", count: StrictJSON.maxDepth + 2) + String(repeating: "]", count: StrictJSON.maxDepth + 2)
        #expect(throws: StrictJSON.ParseError.self) { try StrictJSON.parse(deep) }
        let ok = String(repeating: "[", count: 50) + String(repeating: "]", count: 50)
        #expect(throws: Never.self) { try StrictJSON.parse(ok) }
    }

    @Test("serializes deterministically with sorted keys and escapes")
    func serializes() throws {
        let value: JSONValue = ["b": 1, "a": ["x\n\"y": .null, "n": .double(0.25)], "c": [true, "é"]]
        #expect(value.jsonString == #"{"a":{"n":0.25,"x\n\"y":null},"b":1,"c":[true,"é"]}"#)
        #expect(try StrictJSON.parse(value.serialized()) == value)
    }

    @Test("non-finite doubles serialize as null; integral doubles as integers")
    func numbers() {
        #expect(JSONValue.double(.nan).jsonString == "null")
        #expect(JSONValue.double(.infinity).jsonString == "null")
        #expect(JSONValue.double(3).jsonString == "3")
        #expect(JSONValue.double(-0.5).jsonString == "-0.5")
        #expect(JSONValue.double(1e-7).jsonString == "1e-07")
    }

    @Test("control characters are escaped on output")
    func controlCharacters() throws {
        let value = JSONValue.string("a\u{01}b\u{1F}")
        #expect(value.jsonString == #""a\u0001b\u001f""#)
        #expect(try StrictJSON.parse(value.jsonString) == value)
    }

    @Test("accessors convert between integer and double where exact")
    func accessors() {
        #expect(JSONValue.int(3).doubleValue == 3)
        #expect(JSONValue.double(4).intValue == 4)
        #expect(JSONValue.double(4.5).intValue == nil)
        #expect(JSONValue.string("x").intValue == nil)
    }

    @Test("Codable bridging round-trips through JSONEncoder/JSONDecoder")
    func codable() throws {
        let value: JSONValue = ["k": [1, 2.5, "s", false, .null]]
        let data = try JSONEncoder().encode(value)
        #expect(try JSONDecoder().decode(JSONValue.self, from: data) == value)
    }
}
