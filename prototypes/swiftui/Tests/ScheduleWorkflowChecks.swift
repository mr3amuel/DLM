import Foundation

@MainActor
enum ScheduleWorkflowChecks {
    static func run(model: DownloadModel) async throws {
        let decoder = JSONDecoder()
        decoder.keyDecodingStrategy = .convertFromSnakeCase
        func entry(_ id: String, pending: Bool, missed: Bool = false, once: Bool = true,
                   kind: String = "Download", download: String = "existing-id", next: Int64? = 2000) -> [String: Any] {
            ["rule": ["id": id, "name": id, "enabled": true, "kind": kind, "download_id": download,
                "at_ms": once ? 1_800_000_000_123 as Any : NSNull(), "weekdays": once ? 0 : 127,
                "start_minute": 1320, "end_minute": 120, "revision": 7],
             "pending": pending, "next_run_ms": next as Any? ?? NSNull(), "held": false, "status": "Waiting for schedule",
             "missed_at_ms": missed ? 123 as Any : NSNull(), "notify_missed": missed]
        }
        var rules = [entry("future", pending: true), entry("handled", pending: false),
            entry("completed", pending: false), entry("missed", pending: true, missed: true, next: nil),
            entry("dismissed", pending: true), entry("weekly", pending: true, once: false),
            entry("active-weekly", pending: false, once: false), entry("bandwidth", pending: true, once: false, kind: "Bandwidth")]
        let snapshot = try decoder.decode(ScheduleSnapshot.self, from: JSONSerialization.data(
            withJSONObject: ["rules": rules, "manual_override": false]))
        precondition(Set(snapshot.pendingDownloads.map(\.id)) == ["future", "missed", "dismissed", "weekly"])
        precondition(snapshot.pendingDownloads.first?.id == "missed")
        precondition(snapshot.missedDownloads.map(\.id) == ["missed"])
        precondition(snapshot.missedOccurrences.count == 1 && snapshot.missedOccurrences[0].timestamp == 123)
        // Editing looks through all rules, including a currently handled/active one.
        let handledOnly = try decoder.decode(ScheduleSnapshot.self, from: JSONSerialization.data(
            withJSONObject: ["rules": [rules[1]], "manual_override": false]))
        precondition(handledOnly.editor(forDownload: "existing-id").rule?.id == "handled")
        precondition(handledOnly.editor(forDownload: "different").rule == nil)

        let original = snapshot.rules.first { $0.id == "future" }!.rule
        let utc = TimeZone(secondsFromGMT: 0)!
        let newZone = TimeZone(secondsFromGMT: 9 * 3600)!
        var draft = NativeScheduleDraft(rule: original, timezone: utc)
        var spec = try draft.input(original: original, timezone: newZone)
        precondition(spec.id == original.id && spec.expectedRevision == 7)
        precondition(spec.atMs == original.atMs && spec.date == nil, "Name-only edit changed the absolute instant")
        draft.name = "Updated name"
        spec = try draft.input(original: original, timezone: newZone)
        precondition(spec.atMs == original.atMs && spec.id == original.id)
        draft.start = draft.start.addingTimeInterval(3600)
        spec = try draft.input(original: original, timezone: utc)
        precondition(spec.atMs == nil && spec.date != nil && spec.weekdays == 0)

        let weekly = snapshot.rules.first { $0.id == "weekly" }!.rule
        var weeklyDraft = NativeScheduleDraft(rule: weekly, timezone: utc)
        spec = try weeklyDraft.input(original: weekly, timezone: newZone)
        precondition(spec.date == nil && spec.atMs == nil && spec.weekdays == 127)
        precondition(spec.startMinute == 1320 && spec.endMinute == 120, "Weekly wall time changed with timezone")
        precondition(spec.id == weekly.id && spec.expectedRevision == weekly.revision)
        weeklyDraft.followSystemTimezone(timezone: newZone)
        var local = Calendar(identifier: .gregorian)
        local.timeZone = newZone
        precondition(local.component(.hour, from: weeklyDraft.start) == 22)
        precondition(local.component(.hour, from: weeklyDraft.end) == 2)
        let rebased = try weeklyDraft.input(original: weekly, timezone: newZone)
        precondition(rebased.startMinute == 1320 && rebased.endMinute == 120)
        // Continue validation in the form's current local zone.
        weeklyDraft.weekdays = 0
        do { _ = try weeklyDraft.input(original: weekly, timezone: newZone); fatalError("Accepted no weekdays") }
        catch is EngineFailure {}
        weeklyDraft.weekdays = 1
        weeklyDraft.end = weeklyDraft.start
        do { _ = try weeklyDraft.input(original: weekly, timezone: newZone); fatalError("Accepted equal window endpoints") }
        catch is EngineFailure {}
        var newDraft = NativeScheduleDraft(now: Date(timeIntervalSince1970: 1_800_000_000), timezone: utc)
        spec = try newDraft.input(timezone: utc)
        precondition(spec.id.isEmpty && spec.expectedRevision == nil && spec.date != nil && spec.atMs == nil)
        newDraft.setOneTime(false)
        spec = try newDraft.input(timezone: utc)
        precondition(spec.id.isEmpty && spec.date == nil && spec.atMs == nil && spec.weekdays == 127)
        let switchTime = Date(timeIntervalSince1970: 1_800_000_000)
        newDraft.setOneTime(true, now: switchTime)
        precondition(newDraft.start == switchTime.addingTimeInterval(3600))
        spec = try newDraft.input(timezone: utc)
        precondition(spec.date?.year == 2027)

        // Exercise model read/commit contracts through the real Swift bridge and C seam.
        // At this point the controllable observation clock is asleep, so fixture changes cannot race reads.
        let envelope = try JSONSerialization.data(withJSONObject: ["data": ["rules": rules, "manual_override": false]])
        String(decoding: envelope, as: UTF8.self).withCString { lifecycle_schedule_json($0) }
        try await model.refreshSchedules()
        precondition(model.schedules?.missedDownloads.count == 1)
        let encoder = JSONEncoder()
        encoder.keyEncodingStrategy = .convertToSnakeCase
        let command = EngineCommand(operation: .dismissMissed, occurrences: model.schedules!.missedOccurrences)
        let wire = try JSONSerialization.jsonObject(with: encoder.encode(command)) as! [String: Any]
        let keys = wire["occurrences"] as! [[Any]]
        precondition(keys[0][0] as? String == "missed" && keys[0][1] as? Int == 123)
        await model.dismissMissedSchedules(model.schedules!.missedOccurrences)
        precondition(lifecycle_count(Int32(TEST_DISMISS)) == 1 && lifecycle_count(Int32(TEST_RESUME)) == 0)
        rules[3]["notify_missed"] = false
        let dismissed = try JSONSerialization.data(withJSONObject: ["data": ["rules": rules, "manual_override": false]])
        String(decoding: dismissed, as: UTF8.self).withCString { lifecycle_schedule_json($0) }
        try await model.refreshSchedules()
        precondition(model.schedules?.missedDownloads.isEmpty == true)
        precondition(model.schedules?.pendingDownloads.contains { $0.id == "missed" } == true)

        lifecycle_fail(Int32(TEST_SCHEDULES), 1)
        let saved = try await model.saveSchedule(spec)
        precondition(saved == "schedule-id" && lifecycle_count(Int32(TEST_SAVE)) == 1)
        try await model.refreshSchedules()
        precondition(lifecycle_count(Int32(TEST_SAVE)) == 1, "Retried a committed schedule after a read failure")
        // Start Now is explicit and uses the existing safe resume command.
        await model.action(.resume, id: "existing-id")
        precondition(lifecycle_count(Int32(TEST_RESUME)) == 1)
        lifecycle_schedule_json(nil)
        try await model.refreshSchedules()
        print("Schedule workflow checks passed: pending/missed classification, existing edits, local/absolute time, weekly creation, dismissal and committed-save recovery")
    }
}
