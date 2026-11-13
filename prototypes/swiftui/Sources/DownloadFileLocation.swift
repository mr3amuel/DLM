import AppKit

enum DownloadFileLocation {
    static func target(_ row: Transfer, readable: (URL, Bool) -> Bool = { url, directory in
        var isDirectory: ObjCBool = false
        return FileManager.default.fileExists(atPath: url.path, isDirectory: &isDirectory)
            && (!directory || isDirectory.boolValue) && FileManager.default.isReadableFile(atPath: url.path)
    }) -> URL? {
        func url(_ path: String) -> URL? { path.hasPrefix("/") ? URL(fileURLWithPath: path) : nil }
        if row.status == .completed, let file = url(row.destination), readable(file, false) { return file }
        if row.status != .completed, let partial = row.partialPath.flatMap(url), readable(partial, false) { return partial }
        let folders = [row.destination, row.status == .completed ? nil : row.partialPath].compactMap { $0 }.compactMap(url).map { $0.deletingLastPathComponent() }
        return folders.first { readable($0, true) }
    }
}
