import Foundation
import PeekCore

/// Turns what the Carbon typed into an answer for a choice, slider or range ask
/// (`answer.via = "keyboard"`). Voice answers are matched by peekd (§1.9.5); typed answers
/// are matched here so the bubble can answer at once or say "Didn't match an option".
///
/// Single choice: the option's label or id, its number ("2", "#2"), an ordinal ("second", "2nd",
/// "the last one"), or an unambiguous prefix or whole word of one label. Multiple choice: the
/// same, separated by commas, "and", "&" or "+". Slider: a number inside the bounds (snapped to
/// the step). Range: two numbers ("20-80", "20 to 80", "between 20 and 80"). Text: the text.
public enum TypedAnswerMatcher {
    public static func match(_ typed: String, ask: AskPayload) -> AskValue? {
        let text = typed.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !text.isEmpty else { return nil }
        switch ask.kind {
        case .text:
            return .text(text)
        case .singleChoice(let options):
            return matchOption(text, options).map(AskValue.choice)
        case .multipleChoice(let options, let min, let max):
            let parts = splitList(text)
            var ids: [String] = []
            for part in parts {
                guard let id = matchOption(part, options) else { return nil }
                if !ids.contains(id) { ids.append(id) }
            }
            guard (min...max).contains(ids.count) else { return nil }
            ids.sort { a, b in (options.firstIndex { $0.id == a } ?? 0) < (options.firstIndex { $0.id == b } ?? 0) }
            return .choices(ids)
        case .slider(let spec):
            let numbers = numbersIn(text)
            guard numbers.count == 1, let value = snap(numbers[0], min: spec.min, max: spec.max, step: spec.step) else {
                return nil
            }
            return .number(value)
        case .range(let spec):
            let numbers = numbersIn(text)
            guard numbers.count == 2 else { return nil }
            let lower = Swift.min(numbers[0], numbers[1]), upper = Swift.max(numbers[0], numbers[1])
            guard let a = snap(lower, min: spec.min, max: spec.max, step: spec.step),
                let b = snap(upper, min: spec.min, max: spec.max, step: spec.step)
            else { return nil }
            return .range(lower: a, upper: b)
        }
    }

    /// Snaps `value` to `min + k × step`; nil when it lies outside `min...max`.
    public static func snap(_ value: Double, min: Double, max: Double, step: Double) -> Double? {
        guard value.isFinite, value >= min - step * 0.5, value <= max + step * 0.5 else { return nil }
        return clampAndSnap(value, min: min, max: max, step: step)
    }

    /// Clamps into `min...max` and snaps to the step grid (used by the slider drag too).
    public static func clampAndSnap(_ value: Double, min: Double, max: Double, step: Double) -> Double {
        let clamped = Swift.min(Swift.max(value, min), max)
        guard step > 0 else { return clamped }
        let steps = ((clamped - min) / step).rounded()
        let snapped = min + steps * step
        // Remove binary noise (0.1 + 0.2) by rounding to the step's decimals.
        let decimals = decimalPlaces(of: step)
        let factor = pow(10, Double(decimals))
        return Swift.min(Swift.max((snapped * factor).rounded() / factor, min), max)
    }

    /// Decimal places needed to show values on a `step` grid (0…4).
    public static func decimalPlaces(of step: Double) -> Int {
        guard step.isFinite, step > 0 else { return 0 }
        for places in 0...4 {
            let scaled = step * pow(10, Double(places))
            if abs(scaled - scaled.rounded()) < 1e-9 { return places }
        }
        return 4
    }

    // MARK: Options

    static let ordinals: [String: Int] = [
        "first": 1, "1st": 1, "one": 1, "second": 2, "2nd": 2, "two": 2, "third": 3, "3rd": 3, "three": 3,
        "fourth": 4, "4th": 4, "four": 4, "fifth": 5, "5th": 5, "five": 5, "sixth": 6, "6th": 6, "six": 6,
    ]

    static func matchOption(_ raw: String, _ options: [AskOption]) -> String? {
        let text = normalize(raw)
        guard !text.isEmpty else { return nil }
        let labels = options.map { normalize($0.label) }
        // Exact label or id.
        if let index = labels.firstIndex(of: text) { return options[index].id }
        if let option = options.first(where: { $0.id.lowercased() == text }) { return option.id }
        // Number or ordinal: "2", "#2", "option 2", "the second one", "the last one", "one".
        let allWords = text.split(separator: " ").map(String.init)
        let filler: Set<String> = ["the", "option", "number", "choice", "no", "one", "#"]
        let meaningful = allWords.filter { !filler.contains($0) }
        let words = meaningful.isEmpty ? allWords.filter { $0 != "the" } : meaningful
        if words.count == 1 {
            var word = words[0]
            if word.hasPrefix("#") { word.removeFirst() }
            if let n = Int(word) ?? ordinals[word], (1...options.count).contains(n) { return options[n - 1].id }
            if word == "last" { return options.last?.id }
        }
        // Unique prefix of a label (at least 2 characters).
        if text.count >= 2 {
            let prefixed = labels.indices.filter { labels[$0].hasPrefix(text) }
            if prefixed.count == 1 { return options[prefixed[0]].id }
        }
        // Unique whole-word containment either way ("delete it" → "Delete").
        let typedWords = Set(text.split(separator: " ").map(String.init))
        let containing = labels.indices.filter { index in
            let labelWords = Set(labels[index].split(separator: " ").map(String.init))
            return !labelWords.isEmpty && (labelWords.isSubset(of: typedWords) || typedWords.isSubset(of: labelWords))
        }
        if containing.count == 1 { return options[containing[0]].id }
        return nil
    }

    static func splitList(_ text: String) -> [String] {
        var parts = [text]
        for separator in [",", ";", "&", "+", " and "] {
            parts = parts.flatMap { $0.components(separatedBy: separator) }
        }
        return parts.map { $0.trimmingCharacters(in: .whitespaces) }.filter { !$0.isEmpty }
    }

    /// Lowercased, accents removed, punctuation (except # and -) turned into spaces, spaces collapsed.
    static func normalize(_ text: String) -> String {
        let folded = text.folding(options: [.caseInsensitive, .diacriticInsensitive, .widthInsensitive], locale: nil)
            .lowercased()
        let mapped = folded.unicodeScalars.map { scalar -> Character in
            if CharacterSet.alphanumerics.contains(scalar) || scalar == "#" || scalar == "-" { return Character(scalar) }
            return " "
        }
        return String(mapped).split(separator: " ").joined(separator: " ")
    }

    // MARK: Numbers

    /// Numbers in `text`: "1,500", "-3.5", "20-80" (a dash between numbers separates them), "42%".
    static func numbersIn(_ text: String) -> [Double] {
        var result: [Double] = []
        var current = ""
        let characters = Array(text)
        func flush() {
            let cleaned = current.replacingOccurrences(of: ",", with: "")
            if let value = Double(cleaned), value.isFinite { result.append(value) }
            current = ""
        }
        for (index, character) in characters.enumerated() {
            if character.isNumber || character == "." {
                current.append(character)
            } else if character == ",", !current.isEmpty, index + 1 < characters.count, characters[index + 1].isNumber {
                current.append(character)
            } else if character == "-" || character == "−" {
                // A minus sign only where a number starts ("-3", "between -5 and 5"), not in "20-80".
                let nextIsDigit = index + 1 < characters.count && characters[index + 1].isNumber
                let previousIsDigit = index > 0 && (characters[index - 1].isNumber || characters[index - 1] == ".")
                flush()
                if nextIsDigit && !previousIsDigit { current = "-" }
            } else {
                flush()
            }
        }
        flush()
        return result
    }
}
