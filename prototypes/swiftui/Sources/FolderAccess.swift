import AppKit

@MainActor
final class FolderAccess {
    private let store: URL
    private var bookmarks: [String: Data]
    private var active: [String: URL] = [:]

    init(root: URL) throws {
        store = root.appendingPathComponent("folder-bookmarks.plist")
        do {
            bookmarks = try PropertyListDecoder().decode([String: Data].self, from: Data(contentsOf: store))
        } catch let error as CocoaError where error.code == .fileReadNoSuchFile {
            bookmarks = [:]
        }
    }

    func restore() throws {
        for (path, data) in bookmarks where active[path] == nil {
            var stale = false
            do {
                let url = try URL(resolvingBookmarkData: data, options: [.withSecurityScope, .withoutUI],
                                  relativeTo: nil, bookmarkDataIsStale: &stale)
                guard url.path == path, url.startAccessingSecurityScopedResource() else { throw unavailable(path) }
                active[path] = url
                if stale { try persist(url, path: path) }
            } catch {
                release()
                throw unavailable(path)
            }
        }
    }

    func remember(_ selected: URL) throws -> String {
        // Open panels have already started the implicit access. Convert that grant
        // to an explicit app-scoped bookmark and take ownership of its scope.
        defer { selected.stopAccessingSecurityScopedResource() }
        let path = selected.path
        if active[path] != nil { return path }
        let data = try selected.bookmarkData(options: .withSecurityScope)
        var stale = false
        let scoped = try URL(resolvingBookmarkData: data, options: [.withSecurityScope, .withoutUI],
                             relativeTo: nil, bookmarkDataIsStale: &stale)
        guard scoped.path == path, scoped.startAccessingSecurityScopedResource() else { throw unavailable(path) }
        do {
            try persist(scoped, path: path)
            active[path] = scoped
            return path
        } catch {
            scoped.stopAccessingSecurityScopedResource()
            throw error
        }
    }

    private func persist(_ url: URL, path: String) throws {
        var updated = bookmarks
        updated[path] = try url.bookmarkData(options: .withSecurityScope)
        try FileManager.default.createDirectory(at: store.deletingLastPathComponent(), withIntermediateDirectories: true)
        try PropertyListEncoder().encode(updated).write(to: store, options: .atomic)
        bookmarks = updated
    }

    func release() {
        for url in active.values { url.stopAccessingSecurityScopedResource() }
        active.removeAll()
    }

    private func unavailable(_ path: String) -> EngineFailure {
        EngineFailure(code: "folder_access",
                      message: "DLM cannot restore access to \(path). Make the original folder available or reselect it in Settings, then reopen DLM.")
    }
}
