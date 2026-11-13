import Foundation

struct DownloadListOptions: Equatable {
    enum Sort: String, CaseIterable, Identifiable {
        case creationDate = "Creation Date", filename = "Filename", size = "File Size", speed = "Download Speed"
        case status = "Status", progress = "Progress", priority = "Priority"
        var id: Self { self }
    }
    var sort: Sort = .creationDate
    var ascending = false
    var status: Transfer.Status?
    var minimumBytes: UInt64?
    var maximumBytes: UInt64?

    mutating func applyHeaderSort(_ sort: Sort, ascending: Bool) {
        self.sort = sort
        self.ascending = ascending
    }

    mutating func clearFilters() {
        status = nil
        minimumBytes = nil
        maximumBytes = nil
    }

    mutating func setSizeBounds(minimum: String, maximum: String) throws {
        let low = try NativeNumber.scaled(minimum, places: 6, allowZero: true)
        let high = try NativeNumber.scaled(maximum, places: 6, allowZero: true)
        guard low == nil || high == nil || low! <= high! else {
            throw EngineFailure(code: "invalid", message: NativePreferences.text("Minimum size must not exceed maximum size"))
        }
        minimumBytes = low
        maximumBytes = high
    }

    func select(_ source: [Transfer], search: String = "", category: String? = nil,
                including: (Transfer) -> Bool = { _ in true }) -> [Transfer] {
        let query = search.trimmingCharacters(in: .whitespacesAndNewlines).lowercased()
        return source.filter { row in
            including(row) && (status == nil || row.status == status)
                && (category == nil || (row.categoryId ?? "other") == category)
                && (minimumBytes == nil || row.total.map { $0 >= minimumBytes! } == true)
                && (maximumBytes == nil || row.total.map { $0 <= maximumBytes! } == true)
                && (query.isEmpty || row.filename.lowercased().contains(query) || row.url.lowercased().contains(query))
        }.sorted { a, b in
            // Unknown values remain last in BOTH directions; IDs break ties ascending.
            func compare<T: Comparable>(_ lhs: T?, _ rhs: T?) -> Int {
                if lhs == nil && rhs != nil { return 2 }
                if lhs != nil && rhs == nil { return -2 }
                guard let lhs, let rhs else { return 0 }
                return lhs == rhs ? 0 : (lhs < rhs ? -1 : 1)
            }
            let order: Int
            switch sort {
            case .creationDate: order = compare(a.createdAtMs, b.createdAtMs)
            case .filename: order = compare(a.filename.lowercased(), b.filename.lowercased())
            case .size: order = compare(a.total, b.total)
            case .speed: order = compare(a.speed.isFinite ? a.speed : nil, b.speed.isFinite ? b.speed : nil)
            case .status: order = compare(Self.statusOrder(a.status), Self.statusOrder(b.status))
            case .progress: order = compare(a.fraction, b.fraction)
            case .priority: order = compare(a.priority.tableRank, b.priority.tableRank)
            }
            if order == 2 || order == -2 { return order < 0 }
            return order == 0 ? a.id < b.id : (ascending ? order < 0 : order > 0)
        }
    }
    private static func statusOrder(_ status: Transfer.Status) -> Int {
        switch status {
        case .running: 0
        case .queued: 1
        case .finalizing: 2
        case .paused: 3
        case .ready: 4
        case .completed: 5
        case .failed: 6
        case .cancelled: 7
        }
    }
}

enum DownloadListSelection {
    static func visible(_ ids: Set<String>, in rows: [Transfer]) -> Set<String> {
        ids.intersection(rows.map(\.id))
    }
    static func range(from anchor: String, to id: String, in rows: [Transfer]) -> Set<String>? {
        guard let start = rows.firstIndex(where: { $0.id == anchor }),
              let end = rows.firstIndex(where: { $0.id == id }) else { return nil }
        return Set(rows[min(start, end)...max(start, end)].map(\.id))
    }
    static func next(_ cursor: String?, delta: Int, in rows: [Transfer]) -> String? {
        guard !rows.isEmpty else { return nil }
        let index = cursor.flatMap { id in rows.firstIndex(where: { $0.id == id }) }
        return rows[max(0, min(rows.count - 1, (index ?? (delta > 0 ? -1 : rows.count)) + delta))].id
    }
}

extension TransferPriority {
    var tableRank: Int { switch self { case .high: 0; case .normal: 1; case .low: 2 } }
}
