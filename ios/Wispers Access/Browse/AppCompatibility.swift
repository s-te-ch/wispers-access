import WispersAccessSdk

/// Checks whether this device is capable of opening the app. Returns nil if it
/// is, a description of the problem if it isn't.
///
/// Concretely, a host node newer than this app can share apps in a new, unknown
/// mode it doesn't know, or with new, unknown origins it can't parse.
///
/// Additionally, on iOS, loopback origins go through the SDK's loopback
/// forwarding, which only forwards `http`, so an app with an `https` origin on
/// `localhost` can't be opened here.
func whyUnavailable(_ app: SharedApp) -> String? {
    switch app.mode {
    case .reverseProxy:
        return nil
    case .forwardProxy:
        guard app.startUrl != nil, let targets = proxiedTargets(app: app) else {
            return needsNewerApp
        }
        if targets.contains(where: { $0.loopback && $0.https }) {
            return "Uses https on localhost, which iPhone and iPad can't reach"
        }
        return nil
    case .unknown:
        return needsNewerApp
    }
}

private let needsNewerApp = "Needs a newer version of Wispers Access"
