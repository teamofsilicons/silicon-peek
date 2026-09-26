import Foundation

/// Public documentation anchors cited by validation and runtime messages (headings of docs/drawing.md, slugged the
/// way the site does it). Internal spec sections are never cited: a Silicon cannot open them.
public enum DrawingDocs {
    public static let page = "https://peek.teamofsilicons.com/docs/drawing"
    public static let glass = page + "#glass-what-is-cheap-and-what-is-not"
    public static let limits = page + "#limits"
    public static let rules = page + "#rules-for-good-drawings"
    public static let validation = page + "#validation"
    public static let notSupported = page + "#not-supported"
    public static let scriptStructure = page + "#script-structure"
}

/// Makes a QuickJS stack readable for the Silicon (CLI failure block, `drawing.error`): drops peek's own prelude
/// frames, shortens anonymous frames to `at file:line:col`, and puts the offending source line with a caret under
/// the first frame that points into the script.
///
/// ```text
/// at cassette.js:18:34
///   const tint = art.colors.dominant ?? '#e8d9b8'
///                    ^
/// ```
enum DrawingStack {
    static let maxExcerpt = 120

    static func clean(_ stack: String?, source: String?, filename: String) -> String? {
        guard let stack else { return nil }
        var out: [String] = []
        var excerptAdded = false
        for raw in stack.split(whereSeparator: \.isNewline) {
            var line = raw.trimmingCharacters(in: .whitespaces)
            guard !line.isEmpty, !line.contains(Prelude.filename) else { continue }
            let anonymous = "at <anonymous> ("
            if line.hasPrefix(anonymous), line.hasSuffix(")") {
                line = "at " + line.dropFirst(anonymous.count).dropLast()
            }
            out.append(line)
            if !excerptAdded, let source, let (row, column) = location(in: line, filename: filename),
               let excerpt = excerpt(source: source, line: row, column: column) {
                out.append(contentsOf: excerpt)
                excerptAdded = true
            }
        }
        return out.isEmpty ? nil : out.joined(separator: "\n")
    }

    /// `(line, column)` of `filename:L:C` in one stack line, both 1-based.
    static func location(in line: String, filename: String) -> (Int, Int)? {
        guard let range = line.range(of: filename + ":") else { return nil }
        let rest = line[range.upperBound...]
        let digitsLine = rest.prefix { $0.isNumber }
        guard let row = Int(digitsLine) else { return nil }
        let afterLine = rest.dropFirst(digitsLine.count)
        guard afterLine.first == ":" else { return (row, 0) }
        let digitsColumn = afterLine.dropFirst().prefix { $0.isNumber }
        return (row, Int(digitsColumn) ?? 0)
    }

    /// The source line (leading indentation removed, long lines windowed around the column) and a caret line,
    /// each indented by two spaces.
    static func excerpt(source: String, line: Int, column: Int) -> [String]? {
        let lines = source.split(separator: "\n", omittingEmptySubsequences: false)
        guard line >= 1, line <= lines.count else { return nil }
        var text = Array(lines[line - 1])
        if text.last == "\r" { text.removeLast() }
        let indent = text.prefix { $0 == " " || $0 == "\t" }.count
        var body = Array(text.dropFirst(indent))
        while let last = body.last, last == " " || last == "\t" { body.removeLast() }
        guard !body.isEmpty else { return nil }
        var caret = column > 0 ? column - 1 - indent : -1
        var prefix = "", suffix = ""
        if body.count > maxExcerpt {
            let start = max(0, min(body.count - maxExcerpt, (caret >= 0 ? caret : 0) - maxExcerpt / 2))
            let end = min(body.count, start + maxExcerpt)
            if start > 0 { prefix = "…" }
            if end < body.count { suffix = "…" }
            body = Array(body[start..<end])
            if caret >= 0 { caret = caret - start + prefix.count }
        }
        let excerpt = "  " + prefix + String(body) + suffix
        guard caret >= 0, caret <= prefix.count + body.count else { return [excerpt] }
        return [excerpt, "  " + String(repeating: " ", count: caret) + "^"]
    }
}
