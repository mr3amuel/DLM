import Foundation
@main struct IdentityChecks {
    static func main() {
        let support = URL(fileURLWithPath: "/disposable/Library/Application Support", isDirectory: true)
        let dev = NativeIdentity.storageRoot(support: support, production: false)
        let prod = NativeIdentity.storageRoot(support: support, production: true)
        precondition(dev != prod)
        precondition(dev.lastPathComponent == "com.dlm.native-development")
        precondition(prod.lastPathComponent == "com.dlm.native")
        for _ in 0..<10 { precondition(NativeIdentity.storageRoot(support: support, production: true) == prod) }
        precondition(prod.appendingPathComponent("Engine").path == "/disposable/Library/Application Support/com.dlm.native/Engine")
        print("Identity checks passed: stable production root, isolated development root; no user data accessed")
    }
}
