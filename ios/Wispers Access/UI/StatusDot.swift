import SwiftUI

/// Availability indicator: green when reachable, grey when offline or unknown,
/// red when the share is terminally gone, a small spinner while checking.
/// Mirrors the Android `StatusDot`.
struct StatusDot: View {
    let availability: Availability
    var size: CGFloat = 8

    var body: some View {
        switch availability {
        case .checking:
            ProgressView()
                .controlSize(.mini)
                .frame(width: size, height: size)
        case .online:
            Circle().fill(AccessColor.online).frame(width: size, height: size)
        case .offline, .unknown:
            Circle().fill(AccessColor.outline).frame(width: size, height: size)
        case .removed, .revoked:
            Circle().fill(AccessColor.destructive).frame(width: size, height: size)
        }
    }
}
