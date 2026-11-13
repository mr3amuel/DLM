import Foundation

// Accept ASCII, Persian and Arabic-Indic digits; Arabic decimal mark,
// no grouping separators, signs, exponents, or silent rounding/overflow.
enum NativeNumber {
    static func normalize(_ text: String) -> String {
        String(String.UnicodeScalarView(text.unicodeScalars.map { scalar in
            switch scalar.value {
            case 0x06F0...0x06F9: UnicodeScalar(scalar.value - 0x06F0 + 48)!
            case 0x0660...0x0669: UnicodeScalar(scalar.value - 0x0660 + 48)!
            case 0x066B: UnicodeScalar(46)!
            default: scalar
            }
        }))
    }

    static func scaled(_ text: String, places: Int, allowZero: Bool) throws -> UInt64? {
        let normalized = normalize(text.trimmingCharacters(in: .whitespacesAndNewlines))
        if normalized.isEmpty { return nil }
        let parts = normalized.split(separator: ".", omittingEmptySubsequences: false)
        let scale = (0..<places).reduce(UInt64(1)) { value, _ in value * 10 }
        let validDigits: (Substring) -> Bool = { !$0.isEmpty && $0.utf8.allSatisfy { (48...57).contains($0) } }
        let failure = EngineFailure(code: "invalid", message: NativePreferences.text("Enter a valid nonnegative number without grouping separators."))
        guard (1...2).contains(parts.count), validDigits(parts[0]),
              let whole = UInt64(parts[0]), whole <= UInt64.max / scale else { throw failure }
        let fraction = parts.count == 2 ? parts[1] : "0"
        guard validDigits(fraction), fraction.count <= places,
              let tail = UInt64(String(fraction).padding(toLength: places, withPad: "0", startingAt: 0)),
              whole * scale <= UInt64.max - tail else { throw failure }
        let value = whole * scale + tail
        guard allowZero || value > 0 else { throw failure }
        return value
    }

    static func text(_ value: UInt64?, places: Int) -> String {
        guard let value else { return "" }
        let scale = (0..<places).reduce(UInt64(1)) { value, _ in value * 10 }
        let tail = value % scale
        if tail == 0 { return String(value / scale) }
        let digits = String(tail)
        let padded = String(repeating: "0", count: places - digits.count) + digits
        return "\(value / scale)." + String(padded.reversed().drop(while: { $0 == "0" }).reversed())
    }
}

struct NativeIntegerFormat: ParseableFormatStyle {
    struct Strategy: ParseStrategy {
        func parse(_ value: String) throws -> UInt32 {
            let value = NativeNumber.normalize(value.trimmingCharacters(in: .whitespacesAndNewlines))
            guard !value.isEmpty, value.utf8.allSatisfy({ (48...57).contains($0) }), let number = UInt32(value) else {
                throw EngineFailure(code: "invalid", message: NativePreferences.text("Enter a whole number."))
            }
            return number
        }
    }
    var parseStrategy: Strategy { Strategy() }
    func format(_ value: UInt32) -> String { NativePreferences.number(UInt64(value)) }
}

struct NativeMillisecondsFormat: ParseableFormatStyle {
    struct Strategy: ParseStrategy {
        func parse(_ value: String) throws -> UInt64 {
            guard let number = try NativeNumber.scaled(value, places: 3, allowZero: true) else {
                throw EngineFailure(code: "invalid", message: NativePreferences.text("Enter a number."))
            }
            return number
        }
    }
    var parseStrategy: Strategy { Strategy() }
    func format(_ value: UInt64) -> String { NativeNumber.text(value, places: 3) }
}
