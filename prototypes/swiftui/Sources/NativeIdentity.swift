import Foundation

/// Build configuration is explicit; never infer storage from version or install path.
enum NativeIdentity {
    static let production = Bundle.main.object(forInfoDictionaryKey: "DLMProduction") as? Bool == true
    static let identifier = production ? "com.dlm.native" : "com.dlm.native-development"
    static let name = production ? "DLM" : "DLM Native Development"
    static let firefoxID = production ? "browser-production@dlm.local" : "browser-native@dlm.local"
    static func storageRoot(support: URL, production: Bool) -> URL {
        support.appendingPathComponent(production ? "com.dlm.native" : "com.dlm.native-development", isDirectory: true)
    }
    static func storageRoot() throws -> URL {
        guard Bundle.main.bundleIdentifier == identifier else {
            throw NSError(domain: "DLMIdentity", code: 1, userInfo: [NSLocalizedDescriptionKey:
                "Application identity configuration is inconsistent. Rebuild DLM; no storage was opened."])
        }
        let support = try FileManager.default.url(for: .applicationSupportDirectory, in: .userDomainMask,
                                                  appropriateFor: nil, create: true)
        return storageRoot(support: support, production: production)
    }
}
