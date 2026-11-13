import SwiftUI

// Shell form values only. Rust remains responsible for wall-time resolution,
// DST, queue admission, missed policy and persistence.
struct NativeScheduleDraft {
    var name = ""
    var enabled = true
    var once = true
    var start: Date
    var end: Date
    var weekdays: UInt8 = 127
    var limit = ""
    private let initialStart: Date
    private let initialEnd: Date
    private var formTimezone: TimeZone

    private static func wallTime(_ minute: Int, timezone: TimeZone) -> Date {
        var calendar = Calendar(identifier: .gregorian)
        calendar.timeZone = timezone
        // Time-only controls need a representative date, not today's possible DST gap.
        for day in 1...28 {
            let fields = DateComponents(year: 2001, month: 1, day: day, hour: minute / 60, minute: minute % 60)
            if let date = calendar.date(from: fields), calendar.component(.hour, from: date) * 60 + calendar.component(.minute, from: date) == minute {
                return date
            }
        }
        return Date(timeIntervalSince1970: Double(minute * 60))
    }

    mutating func setOneTime(_ value: Bool, now: Date = .now) {
        guard once != value else { return }
        once = value
        // A time-only weekly control uses a representative date. Never expose
        // that historical date as the default of a newly selected one-time start.
        if value { start = now.addingTimeInterval(3600) }
    }

    mutating func followSystemTimezone(kind: ScheduleKind = .download, timezone: TimeZone = .autoupdatingCurrent) {
        let zone = TimeZone(identifier: timezone.identifier) ?? timezone
        guard zone != formTimezone else { return }
        if !once || kind == .bandwidth {
            var old = Calendar(identifier: .gregorian)
            old.timeZone = formTimezone
            let from = old.component(.hour, from: start) * 60 + old.component(.minute, from: start)
            let until = old.component(.hour, from: end) * 60 + old.component(.minute, from: end)
            start = Self.wallTime(from, timezone: zone)
            end = Self.wallTime(until, timezone: zone)
        }
        formTimezone = zone
    }

    init(rule: ScheduleRule? = nil, now: Date = .now, timezone: TimeZone = .autoupdatingCurrent) {
        formTimezone = TimeZone(identifier: timezone.identifier) ?? timezone
        func time(_ minute: Int) -> Date { Self.wallTime(minute, timezone: timezone) }
        start = rule?.atMs.map { Date(timeIntervalSince1970: Double($0) / 1000) }
            ?? rule.map { time($0.startMinute) } ?? now.addingTimeInterval(3600)
        end = rule.map { time($0.endMinute) } ?? now.addingTimeInterval(7200)
        initialStart = start
        initialEnd = end
        if let rule {
            name = rule.name
            enabled = rule.enabled
            once = rule.atMs != nil
            weekdays = rule.weekdays
            limit = speedLimitText(rule.bandwidthLimit)
        }
    }

    func input(kind: ScheduleKind = .download, downloadID: String? = nil, original: ScheduleRule? = nil,
               timezone: TimeZone = .autoupdatingCurrent) throws -> ScheduleInput {
        var calendar = Calendar(identifier: .gregorian)
        calendar.timeZone = timezone
        let oneTime = once && kind == .download
        let preserveInstant = oneTime && original?.atMs != nil && start == initialStart
        let date = calendar.dateComponents([.year, .month, .day], from: start)
        func minute(_ time: Date) -> Int { calendar.component(.hour, from: time) * 60 + calendar.component(.minute, from: time) }
        let from = !oneTime && original?.atMs == nil && start == initialStart
            ? original?.startMinute ?? minute(start) : minute(start)
        let until = !oneTime && original?.atMs == nil && end == initialEnd
            ? original?.endMinute ?? minute(end) : minute(end)
        if !oneTime && (weekdays == 0 || weekdays > 127 || from == until) {
            throw EngineFailure(code: "invalid", message: NativePreferences.text("Select weekdays and distinct window start and end times"))
        }
        return ScheduleInput(id: original?.id ?? "", name: name.trimmingCharacters(in: .whitespacesAndNewlines), enabled: enabled,
            kind: kind, downloadId: original?.downloadId ?? downloadID,
            date: oneTime && !preserveInstant ? .init(year: date.year ?? 0, month: date.month ?? 0, day: date.day ?? 0) : nil,
            weekdays: oneTime ? 0 : weekdays, startMinute: from, endMinute: until,
            limit: kind == .bandwidth ? try parseSpeedLimit(limit) : nil,
            atMs: preserveInstant ? original?.atMs : nil, expectedRevision: original?.revision)
    }
}

struct NativeScheduleWeekdays: View {
    @Binding var weekdays: UInt8
    @Environment(\.locale) private var locale
    var body: some View {
        var calendar = Calendar(identifier: .gregorian)
        calendar.locale = locale
        let names = calendar.shortStandaloneWeekdaySymbols
        return HStack(spacing: 5) {
            ForEach(0..<7) { day in
                let mask = UInt8(1 << day)
                Toggle(names[(day + 1) % 7], isOn: Binding(
                    get: { weekdays & mask != 0 },
                    set: { weekdays = $0 ? weekdays | mask : weekdays & ~mask }
                )).toggleStyle(.button).controlSize(.small)
            }
        }
    }
}
