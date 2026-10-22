import CryptoKit
import Foundation
import Network
import WebKit
import WispersAccessSdk

/// The persistent web data store of each share's forward-proxy apps, which
/// keeps the share's cookies and storage apart from every other share's.
enum ShareDataStore {
    /// The share's data store, the same one across launches.
    static func store(for shareID: ShareId) -> WKWebsiteDataStore {
        WKWebsiteDataStore(forIdentifier: identifier(shareID))
    }

    /// Points the share's data store at the forward proxy, with the share's
    /// credential, for the hosts of the share's named origins only. A share
    /// without any goes direct, its loopback origins included.
    static func route(_ store: WKWebsiteDataStore, for share: Share, through proxy: ForwardProxy) throws {
        let hosts = Set(
            share.apps
                .flatMap { proxiedTargets(app: $0) ?? [] }
                .filter { !$0.loopback }
                .map(\.host)
        )
        guard !hosts.isEmpty, let port = NWEndpoint.Port(rawValue: proxy.port()) else {
            store.proxyConfigurations = []
            return
        }
        let credential = try proxy.credential(share: share.id)
        var configuration = ProxyConfiguration(
            httpCONNECTProxy: .hostPort(host: "127.0.0.1", port: port), tlsOptions: nil)
        configuration.applyCredential(username: credential.username, password: credential.password)
        configuration.matchDomains = hosts.sorted()
        store.proxyConfigurations = [configuration]
    }

    /// Deletes the data store of a share that was left, with its cookies and
    /// storage. Fails quietly while a web view still uses it.
    static func remove(_ shareID: ShareId) async {
        try? await WKWebsiteDataStore.remove(forIdentifier: identifier(shareID))
    }

    /// Share IDs are UUIDs, so they serve as the store's identifier directly;
    /// one that isn't gets a UUID made from its hash.
    private static func identifier(_ shareID: ShareId) -> UUID {
        if let uuid = UUID(uuidString: shareID) { return uuid }
        let digest = Array(SHA256.hash(data: Data(shareID.utf8)))
        return digest.withUnsafeBufferPointer { NSUUID(uuidBytes: $0.baseAddress) as UUID }
    }
}
